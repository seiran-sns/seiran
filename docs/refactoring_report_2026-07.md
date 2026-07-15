# seiran コードベース改善レポート（2026-07-15）

リファクタリング・ユニットテストの観点でコードベース全体を診断した結果。
観点は (1) how/what の分離、(2) Worker コンポーネントの活用度、に加えて
専門家として問題と判断した箇所をすべて挙げる。

各項目には ID を振り、末尾の「改修計画」で優先順位と対応状況を管理する。

---

## 総評

リポジトリ層のトレイト化・ハンドラの関数分割など、設計の骨格は整っている。
しかし **「キュー基盤は作ったが、実処理が乗っていない」** のが最大の問題。
ジョブハンドラ 6 種のうち 3 種（OutboundPostDelivery / InboundActivityProcess /
ActorMetadataResolve）は偽 sleep + 成功ログのプレースホルダーのままで、
本来そこへ乗るべき実処理は API ハンドラ内の `tokio::spawn`（30 箇所超）に
書かれている。結果として:

- **スケールアウト不能**: spawn は API プロセスに張り付くため、worker ロールを増やしても負荷が逃げない
- **リトライなし**: spawn 内の配送は一発失敗で消失する（WorkerEngine には指数バックオフが実装済みなのに使われていない）
- **how/what 混在**: 「state から 4 つクローンして spawn し、エラーは eprintln」という配送の how が全ハンドラにコピペされている

また、診断中に **InMemoryJobQueue の優先度キューが同一優先度で LIFO になる実バグ**
（コメントは FIFO 保証と主張）を発見した。ユニットテストがあれば即座に検出できた
種類のバグであり、テスト不足の実害の例である。

---

## A. Worker / ジョブキューの活用（指定観点②）

### A-1. ジョブハンドラ 3/6 が偽実装のまま、実処理は spawn に残っている 【重大】
- `jobs/outbound_post_delivery.rs` / `jobs/inbound_activity_process.rs` /
  `jobs/actor_metadata_resolve.rs` は `sleep(300ms)` + 「配送成功」ログの偽実装。
- 偽の成功ログは運用時の誤診を招く。偽 sleep はテスト時間も無駄にする。
- 実装済みなのは `actor_history_sync` / `atp_repository_publish` / `bsky_video_poll` の 3 種のみ。

### A-2. AP 配送がすべて tokio::spawn 直生やし 【重大・最優先】
- `notes.rs`（投稿・リポスト・リアクションの AP 配送 7 箇所）、`users.rs`
  （Update(Actor) 配送）が spawn 直生やし。`account.rs` の退会処理に至っては
  `deliver_delete_actor` を **同期 await** しており、フォロワー数に比例して
  退会レスポンスが遅延する。
- AP 配送の依存は db / ap_client / local_domain / AP 秘密鍵のみで、すべて
  worker プロセスでも用意できる。**ジョブ化の障害はない。**
- ジョブ化すれば WorkerEngine の指数バックオフ（OutboundPostDelivery 設定:
  最大 10 回・最大 1 時間）に自動で乗り、現状の「一発失敗で消失」が解消する。

### A-3. インバウンド処理があべこべ 【重大】
- `inbox.rs` は既知のアクティビティ 6 種（Follow/Create/Accept/Undo/Announce/Like）を
  spawn で処理し、**未知タイプだけ**キューに積む。しかも積まれた先の
  `InboundActivityProcess` は偽実装なので実質捨てている。
- 本来は逆で、署名検証（低レイテンシ必須）だけ同期で行い、処理本体を
  すべてキューへ積むべき。ただし handle_follow 等（約 1,000 行）が
  federation-inbox クレートの AppState に依存しているため、seiran-common への
  移設を伴う大規模改修になる。→ フェーズ分けして対応（改修計画参照）。

### A-4. split-role 構成でキューを共有できない 【既知の制約】
- `--role worker` 単独起動時は自プロセス専用の InMemoryJobQueue になり、
  api が積んだジョブを処理できない（seiran-server main.rs に明記済み）。
- Redis（または Postgres `FOR UPDATE SKIP LOCKED`）バックエンドが入るまで、
  「Worker を別プロセスで動かせる」はアーキテクチャ上の建前に留まる。
  `Job` enum が Serialize/Deserialize 済みなのは良い準備。キュー永続化は
  リトライ状態の永続化（A-6）も同時に解決する本命の改修。

### A-5. ATP コミットは Worker へ移せない構造的結合がある 【設計課題】
- `AtpCommitService` はコミット後に in-process の broadcast チャネルで
  subscribeRepos WebSocket（api ロール）へフレームを流すため、worker プロセス
  へ移すとリレーへのイベント配信が途切れる。
- 分離するには `atp_repo_events` テーブル経由のポーリング or Redis pub/sub が必要。
  A-4 とセットで設計すべき項目。それまで ATP コミットの spawn は現状維持が正しい。

### A-6. WorkerEngine 自体の課題
- **並列数無制限**: `run()` はデキューしたそばから無制限に spawn する。
  キューが溜まっていた場合に一斉発火する（サンダリングハード）。
- **リトライ状態がキューの外**: リトライ待ちは spawn + sleep で保持され、
  キュー長にも現れず、プロセス再起動で消失する。
- **graceful shutdown なし**。
- いずれも A-4（キュー永続化）とセットで解決するのが効率的。

### A-7. バックフィルの二重実装
- `follows.rs` / `users.rs` の Bsky 過去ログ取り込みは `backfill_bsky_posts` を
  spawn しているが、同等機能のジョブ `ActorHistorySync`（実装済み・ドメイン単位
  並列制限つき）が既に存在する。ジョブ側に一本化すべき。

---

## B. how/what の分離（指定観点①）

### B-1. ap/deliver.rs は同じ how のコピペが 5 回以上 【重大】
- `deliver_ap_announce` / `deliver_delete_actor` / `deliver_undo_announce` /
  `deliver_update_actor` / `deliver_ap_reaction` / `deliver_ap_undo_reaction` は
  すべて「① username 取得（同一 SQL）→ ② フォロワー inbox 一覧取得（同一 SQL）
  → ③ アクティビティ JSON 構築 → ④ 署名 POST ファンアウト + ok/ng 集計ログ」
  の構造で、**各関数の固有部分は ③ だけ**。①②④の how が毎回ベタ書きされている。
- ③ を純関数（`build_announce_activity` 等）に切り出せばユニットテスト可能になり、
  ①②④ は共通ヘルパー 1 組に集約できる。ファイルは 859 行 → 大幅減の見込み。

### B-2. 「state から 4 点クローンして spawn」ボイラープレート
- `db` / `local_domain` / `ap_private_key_pem` / `ap_client` のクローン + spawn +
  eprintln という配送ディスパッチの how が notes.rs だけで 7 回出現。
  A-2 のジョブ化（`enqueue` 一行になる）で同時に解消する。

### B-3. lib.rs spawn_startup_tasks が無関係な 3 処理のベタ書き
- ① Cloudflare TXT 再登録、② requestCrawl、③ #identity バックフィル（生 SQL 入り）
  が 1 つの無名 async ブロックに直列で書かれている。名前付き関数 3 つに分割すべき。

### B-4. classify_post が (bool, bool, bool) タプルを返す
- 「ローカル/Fedi リモート/Bsky リモート」は排他な分類なのに 3 つの bool で表現され、
  呼び出し側は `(_is_local, is_fedi, _is_bsky)` と不要要素を捨てて受けている。
  `enum PostOrigin { LocalOrSeiran, FediRemote, BskyRemote }` にすべき。

### B-5. 引数 9 個の配送関数
- `deliver_regular_post` / `deliver_repost` は `#[allow(clippy::too_many_arguments)]`
  で黙らせている。配送指示（どこへ・何を）を表す構造体にまとめるべき。
  A-2 のジョブペイロード設計と同時に解決する。

### B-6. PLC genesis 登録リトライループのコピペ
- `auth.rs`（register）と `setup.rs`（初回管理者作成）に「3 回リトライ + 失敗時
  TXT 掃除」の同一ロジックが重複している。共通関数に抽出すべき。

### B-7. リポジトリ層があるのに生 SQL がハンドラ層に残存
- `notes.rs` の `fetch_attachments_map` / `embed_renotes` / `fetch_reposted_ids` /
  `fetch_reactions_map`、`lib.rs` の `run_media_gc` / startup バックフィル等。
  PostRepository 等のトレイトが整備されているのに一貫していない。
  （テスト容易性にも直結: リポジトリ経由ならモックで単体テストできる）

---

## C. 診断中に発見した実バグ

### C-1. InMemoryJobQueue が同一優先度で LIFO になる 【バグ】
- `memory.rs` の `Ord` 実装:
  `other.priority.cmp(&self.priority).reverse().then(other.sequence.cmp(&self.sequence).reverse())`
  — sequence 側の `.reverse()` が余計で、max-heap は**最新投入ジョブを先に**取り出す。
  コメントの「同一優先度内での FIFO 保証」に反する。
- 現状は WorkerEngine が全件即 spawn するため実害が顕在化しにくいが、
  並列制限（A-6）を入れた瞬間に飢餓が発生する時限バグ。

### C-2. 読み取り系のエラー握りつぶし
- `fetch_attachments_map` 等が DB エラーを `unwrap_or_default()` で吸収し、
  障害時に「添付なしの正常応答」を返す。少なくともログは残すべきで、
  タイムライン本体の取得失敗と同様に 5xx を返すか方針を統一すべき。

### C-3. AP 秘密鍵未設定時に空文字で署名を試みる
- `state.secrets.ap_private_key_pem.clone().unwrap_or_default()` が各所にあり、
  鍵未設定でも空文字 PEM のまま配送処理へ進み、配送先ごとに署名エラーを吐く。
  fail-fast（起動時 or enqueue 時に検知）にすべき。

---

## D. 一貫性・横断的な問題

### D-1. ログが println!/eprintln! 一色
- `tracing` 未導入。レベル分けも構造化もなく、ジョブ実行系（リトライ回数、
  ジョブ種別等）こそ構造化ログの恩恵が大きい。導入は機械的な置換で済むが
  変更行数が多いため独立フェーズとする。

### D-2. エラーレスポンスの流儀が混在
- 同一ハンドラ内で `ApiError::Internal(...)` と `(StatusCode, "...")` タプルが
  混在。ApiError に統一すべき。

### D-3. 認証 → アクター解決の定型 15 行が全ハンドラにコピペ
- `extract_auth` → `find_local_by_user_id` → 同型のエラーマッピング。
  axum の extractor（`FromRequestParts`）にすれば各ハンドラ 1 引数になる。

### D-4. env::var の読み取りがコード深部に散在
- `AtpCommitService::spawn_request_crawl` の `LOCAL_DOMAIN`、
  `atp_repository_publish` の `ATP_HANDLE` / `ATP_APP_PASSWORD` 等。
  設定は起動時に一括で読み、依存として注入すべき（テスト容易性も上がる）。

### D-5. notes.rs が 1,685 行の god file
- DTO・SQL・バリデーション・配送オーケストレーション・ハンドラが同居。
  A-2 / B-7 の改修で自然に痩せるため、まずそちらを先行し、その後
  `notes/`（dto.rs / queries.rs / handlers.rs）への分割を検討。

---

## E. ユニットテスト

### E-1. テストは純関数のみ 76 個、コア機構はゼロ
- 既存テストは validate_reaction_content / classify_post 等の純関数に偏っており、
  **queue / worker / jobs / repository はテストゼロ**。C-1 のバグはこの穴の実害。

### E-2. テスト可能にする設計はあるのに未活用
- リポジトリ層がトレイトなのでモック注入は可能な構造。にもかかわらず
  ハンドラ直 SQL（B-7）がテストを阻んでいる。

### E-3. 統合テスト基盤なし
- `tests/` ディレクトリが存在しない。まずは testcontainers 等を使わずとも、
  ローカル DB 前提の `#[ignore]` 付き統合テストから始められる。

### E-4. 今回の改修で追加すべきテスト
- InMemoryJobQueue: 優先度順・同一優先度 FIFO・notify（C-1 の回帰テスト）
- WorkerEngine: バックオフ遅延計算（純関数化した上で）
- Job enum: serde 往復（将来の Redis 化に備えた互換性テスト）
- deliver.rs: アクティビティ JSON 構築の純関数群（B-1 で切り出したもの）

---

## 改修計画

### 今回実施（このレポートと同時に改修・**実施済み**）
| # | 項目 | 対象 | 状態 |
|---|------|------|------|
| 1 | C-1 修正 + E-4 のキュー/バックオフテスト | memory.rs, worker.rs | ✅ 完了（テスト10件追加） |
| 2 | B-1: deliver.rs の how/what 分離 + 構築純関数のテスト | ap/deliver.rs | ✅ 完了（共通ヘルパー3種 + build_* 純関数群、テスト11件追加。全滅時のみ Err を返すようにしジョブリトライに対応） |
| 3 | A-2/B-2/B-5: AP 配送のジョブ化（`Job::ApDelivery` + `ApDeliveryKind`、JobContext への `DeliveryConfig`（local_domain/AP 鍵）注入、spawn → enqueue 置換、退会の同期配送も非同期化、C-3 の空文字鍵署名も解消） | traits.rs, worker.rs, jobs/ap_delivery.rs, notes.rs, users.rs, account.rs, seiran-server | ✅ 完了（旧 `OutboundPostDelivery` プレースホルダーは `ApDelivery` 実装に置換。Job enum の serde 往復テスト追加） |
| 4 | A-1: 偽実装プレースホルダーの除去（偽 sleep・偽成功ログ削除、未実装を明示） | jobs/inbound_activity_process.rs, jobs/actor_metadata_resolve.rs | ✅ 完了（ロードマップ 06 のチェックも実態に合わせ修正） |
| 5 | A-7: バックフィルを ActorHistorySync ジョブへ一本化 | follows.rs, users.rs | ✅ 完了（`backfill_bsky_posts` 削除。取得上限は 50件/7日 → ジョブ仕様の 300件/30日 に統一） |
| 6 | B-3: spawn_startup_tasks の分割 | lib.rs | ✅ 完了（TXT 確保 / requestCrawl / #identity バックフィルの3関数に分割） |
| 7 | B-4: classify_post の enum 化 / B-5: 配送引数の構造体化 | notes.rs | ✅ 完了（`PostOrigin` enum、`DeliveryTargets` / `RegularPostDelivery` 構造体） |

> 補足: ATP コミット系の spawn（`atp_service.commit_*`）は A-5 の構造的結合により**意図的に残している**。
> `all` ロールでは API と Worker が同一プロセス・同一キューを共有するため、今回のジョブ化は即時に有効。
> split-role 構成で効かせるには #8（キュー永続化）が必要。

### 次フェーズ（別コミット推奨・要設計判断）
| # | 項目 | 備考 |
|---|------|------|
| 8 | A-4/A-6: キューの永続化バックエンド（Redis or Postgres SKIP LOCKED）+ Worker 並列制限・リトライのキュー内保持 | split-role スケールアウトの本丸 |
| 9 | A-3: インバウンド処理のジョブ化 | inbox ハンドラ群の seiran-common 移設を伴う |
| 10 | A-5: ATP コミットイベントのプロセス間配信 | #8 とセットで設計 |
| 11 | D-1: tracing 導入 | 機械的だが変更行数大 |
| 12 | D-3: 認証 extractor 化 / D-2: ApiError 統一 | |
| 13 | B-6: PLC リトライ共通化 / B-7: 生 SQL のリポジトリ層移設 / D-5: notes.rs 分割 | |
| 14 | E-3: 統合テスト基盤 | |
