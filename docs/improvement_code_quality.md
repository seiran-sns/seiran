# コード品質診断レポート

診断日: 2026-07-02  
対象コミット: c3b0946 (Phase 3 完了時点)

---

## 1. 診断サマリー

| 深刻度 | 件数 |
|--------|------|
| 高     | 2    |
| 中     | 6    |
| 低     | 5    |

「高」は放置すると本番障害・データ不整合に直結する問題。「中」は将来の保守・拡張を妨げるもの。「低」は次回以降のリファクタリングで対処すれば十分なもの。

---

## 2. 改善項目リスト（優先度順）

### [高-1] ATP コミットパイプラインにデータベーストランザクションがない

- **深刻度**: 高
- **該当箇所**: `crates/seiran-common/src/atp/service.rs` — `commit_record_inner()`
- **問題点**:  
  `commit_record_inner` は以下の順番で独立した SQL クエリを発行する。  
  1. `atp_blocks` への INSERT  
  2. `actors` の `at_repo_cid` / `at_repo_rev` UPDATE  
  3. `atp_records` への INSERT  
  4. `atp_repo_events` への INSERT  

  これらはひとつの `BEGIN … COMMIT` に包まれていない。ステップ 2 の後にプロセスがクラッシュした場合、`atp_blocks` にブロックが入っているが `atp_repo_events` にイベントが残らず、WebSocket ブロードキャストも行われない。リレーとの状態が食い違い、手動修正が必要になる。
- **改善案**:  
  `sqlx::PgPool` から `begin().await?` でトランザクションを取得し、全クエリを同一トランザクション内で発行する。最後に `tx.commit().await?` する。

---

### [高-2] federation-inbox ハンドラが Repository 層を経由せず SQL を直書きしている

- **深刻度**: 高
- **該当箇所**: `crates/seiran-federation-inbox/src/handlers/inbox.rs` — `handle_follow()`、`handle_create_note()`
- **問題点**:  
  プロジェクトは `repository/` 配下にトレイトと Pg 実装を用意しているが、inbox ハンドラはその層を使わず `sqlx::query` を直接呼んでいる。特にリモートアクターの upsert SQL が `handle_follow` と `handle_create_note` の両方にほぼ同一のコードとして重複している。`PgActorRepository::upsert_remote_fedi()` が存在するにもかかわらず使われていない。  
  - テストで差し替えが効かない（`Arc<dyn ActorRepository>` を渡せない）  
  - バグ修正を 1 箇所で完結できない
- **改善案**:  
  `AppState` に `Arc<dyn ActorRepository>` と `Arc<dyn PostRepository>` を持たせ（seiran-api 側と同様）、inbox ハンドラはそれを経由するよう修正する。SQL の重複も解消される。

---

### [中-1] ATP コミット毎に全レコードを全件読み込んで MST を再構築している

- **深刻度**: 中
- **該当箇所**: `crates/seiran-common/src/atp/service.rs` — `load_atp_entries()`、`commit_record_inner()`
- **問題点**:  
  投稿が増えるほど MST 再構築のために `posts` テーブルと `atp_records` テーブルから全件取得するクエリが重くなる（O(n) でスキャン増加）。現フェーズでは問題にならないが、ユーザーあたり数千件になると目立ってくる。
- **改善案**:  
  既存のコミット CID を `actors.at_repo_cid` から読み込み、CAR からインクリメンタルに更新する方式（差分 CAR）に切り替えるか、`atp_records` に MST ノードをキャッシュする仕組みを追加する。いずれもフェーズ 5 以降の話なので、今は「既知の制限」としてコメントを追記しておく程度でよい。

---

### [中-2] ジョブキューのハンドラがスタブのまま実処理と二重路線になっている

- **深刻度**: 中
- **該当箇所**:  
  - `crates/seiran-common/src/jobs/outbound_post_delivery.rs`  
  - `crates/seiran-common/src/jobs/inbound_activity_process.rs`
- **問題点**:  
  `outbound_post_delivery::handle()` は 300ms スリープして成功を返すスタブ。実際の AP 配送は `handlers/notes.rs` の `tokio::spawn` で直接行われている。`inbound_activity_process::handle()` も同様のスタブ。  
  つまり「キューに入れる」コードパスと「直接 spawn する」コードパスが両立しており、どちらが正として機能しているか分かりにくい。障害時にリトライが効くのはキュー経由のみのはずだが、実際にはキューが使われていない。
- **改善案**:  
  Phase 4/5 でキュー経由に一本化するまでは、スタブのコメントに「このハンドラは未統合。実処理は `handlers/notes.rs` の spawn にある」と明記する。統合後はスタブを削除し、`handlers/notes.rs` の直接 spawn を削除する。

---

### [解消済み] 構造化ログがなく `eprintln!` で全ログを出力している

- **状態**: 2026-07リファクタリングで解消。ワークスペース全体（272箇所）の `println!`/`eprintln!` を `tracing::info!`/`warn!`/`error!` へ機械的に置換（メッセージ文言中の「失敗」「エラー」等のキーワードでレベルを判定）。`seiran-server/main.rs` で `tracing_subscriber::fmt().with_env_filter(...).init()` を呼び、`RUST_LOG` 環境変数でレベル制御可能になった（未設定時は `info`）。JSON フォーマッタは未導入（今後の課題、`tracing_subscriber::fmt().json()` に切り替えるだけで対応可能）。

---

### [中-4] `ApClient` の公開鍵キャッシュに TTL がない

- **深刻度**: 中
- **該当箇所**: `crates/seiran-common/src/ap/client.rs` — `key_cache: Arc<RwLock<HashMap<String, String>>>`
- **問題点**:  
  リモートサーバーが鍵をローテーションした場合、古い PEM がキャッシュに残り続け、署名検証が恒久的に失敗する。再起動するまで解消しない。
- **改善案**:  
  キャッシュエントリに `Instant` のタイムスタンプを持たせ、一定時間（例: 1 時間）が経過したエントリを再フェッチするか、`moka` 等の TTL 付きキャッシュクレートを採用する。

---

### [中-5] `AtpCommitService::commit_profile` が環境変数を直接読んでいる

- **深刻度**: 中
- **該当箇所**: `crates/seiran-common/src/atp/service.rs` — `commit_profile()` の `std::env::var("LOCAL_DOMAIN")`
- **問題点**:  
  `AtpCommitService` は `AppState` から `local_domain: String` を受け取らず、`commit_profile` 内で `std::env::var("LOCAL_DOMAIN")` を直読みしている。他の全コードは `AppState.local_domain` を使っており一貫性がない。テスト時にも環境変数を設定しなければならず、差し込みにくい。
- **改善案**:  
  `AtpCommitService::new()` に `local_domain: String` を追加し、`commit_profile` は `self.local_domain` を参照するよう修正する。

---

### [中-6] `create_note` ハンドラが添付ファイルの INSERT を Repository を経由せず直叩きしている

- **深刻度**: 中
- **該当箇所**: `crates/seiran-api/src/handlers/notes.rs` — `create_note()` の `post_attachments` INSERT
- **問題点**:  
  添付ファイル紐付けの INSERT のみ `state.db` を直叩きしており、他の操作（`state.posts.insert()` 等）とスタイルが食い違う。`PostRepository` に `attach_media_files(post_id, ids)` のようなメソッドを追加するのが一貫した設計。
- **改善案**:  
  `PostRepository` トレイトに添付ファイル紐付けメソッドを追加し、ハンドラはそれを呼ぶようにする。または添付ファイル専用の `AttachmentRepository` を作る。

---

### [低-1] `mention.rs` の単体テストが実際の変換関数をテストしていない

- **深刻度**: 低
- **該当箇所**: `crates/seiran-common/src/mention.rs` — `email_at_sign_should_not_start_mention` テスト
- **問題点**:  
  既存テストは `convert_mentions_for_bsky` / `convert_mentions_for_ap` を呼ばず、内部ロジックを手動で再現して `@` の前判定だけを確認している。`convert_*` 関数自体のロジックが回帰しても検出できない。
- **改善案**:  
  テスト追加計画（§3）参照。

---

### [低-2] `traits.rs` に現在未使用の型が多数定義されている

- **深刻度**: 低
- **該当箇所**: `crates/seiran-common/src/traits.rs` — `DbPost`、`SearchSession`、`SessionStore`
- **問題点**:  
  将来の機能用にスキャフォールドされているが、現在どこからも参照されていない。コードを読む人が「これは使われているのか？」と調査コストを払う。
- **改善案**:  
  使用タイミングが決まるまでは別ファイル（例: `future/search.rs`）に分離し、`pub mod future` で隔離するか、コメントで「Phase X 実装予定」と明示する。あるいは実装フェーズが近付いたら `#[allow(dead_code)]` を除去して警告を活かす。

---

### [解消済み] `create_job_queue()` が redis 指定でも InMemory を返し誤解を招くログを出す

- **状態**: 2026-07リファクタリングで解消。`RedisJobQueue` を実装し、`create_job_queue(is_monolith)` はロール（モノリス/split-role）と `REDIS_URL` の有無に応じて明示的にバックエンドを選択・ログ出力するようになった（`crates/seiran-common/src/queue/mod.rs`）。`JOB_QUEUE_BACKEND` 環境変数は廃止し `REDIS_URL` の有無で判定する方式に変更。

---

### [低-4] フロントエンド Timeline のタイムライン取得エラーが無視される

- **深刻度**: 低
- **該当箇所**: `frontend/src/pages/Timeline.tsx` — `useEffect` の `fetch.then(setNotes).finally(...)`
- **問題点**:  
  API エラーが `.catch()` / `.then(_, onRejected)` で処理されないため、タイムライン取得失敗時にユーザーには何も表示されず、空のリストのままになる。
- **改善案**:  
  `.catch((err) => setLoadError(getErrorMessage(err)))` を追加し、エラーメッセージを表示する `loadError` 状態を管理する。

---

### [低-5] `parse_signature_header` が値中の `=` で誤分割する可能性がある

- **深刻度**: 低
- **該当箇所**: `crates/seiran-common/src/ap/client.rs` — `parse_signature_header()`
- **問題点**:  
  `part.splitn(2, '=')` で分割しているが、`signature` フィールドの base64 値がパディング `=` で終わる場合（例: `signature="abc=="`）はパディングが value の末尾に含まれるため実害はない。しかし `algorithm` フィールドにイコールが含まれる非標準実装や、将来の `headers` 値拡張に対して壊れる余地がある。また `keyId` に `=` が含まれない限り現状問題ないが、コードのコメントには脆弱性の周知を追記すると良い。
- **改善案**:  
  `splitn(2, '=')` は現状で十分だが、`Signature` ヘッダーの値をより堅牢にパースするには `split_once('=')` を明示的に使い、`trim_matches('"')` で前後のクォートを除去する現行方式の意図をコメントで明文化する。

---

## 3. テスト追加計画

### 3-1. `mention.rs` — 変換関数の純粋ロジック部分のユニットテスト

現在の `email_at_sign_should_not_start_mention` テストは DB・HTTP に触れない範囲の構文確認だが、変換ロジックの大部分（メンションの境界検出・フォールバック動作）は DB や HTTP なしでテストできる。`convert_mentions_for_bsky` を DB/HTTP モックに差し替えるか、境界検出ロジックを独立した純粋関数に切り出してテストする。

追加すべきケース:
- `@alice` が `@alice.example.com` に変換される（ローカルアクター確認が成功する場合の想定）
- `@alice@mastodon.social` の `@` が正しく 2 つの部分に分割される
- `admin@example.com` がメンション扱いされない（メールアドレス）
- テキスト末尾の `@` が単体で残る（後続文字なし）
- `@` の連続 `@@alice` が正しく処理される

### 3-2. `ap/deliver.rs` — `plain_to_html` は既にテスト済み（良好）

既存テストで単段落・複数段落・XSS エスケープが網羅されており、このモジュールは良好。

### 3-3. `atp/repo.rs` — MST の決定論テスト

既存テストで TID 長・アルファベット・CID ラウンドトリップ・空 MST・単エントリは確認済み。追加すべきケース:
- 同一エントリを同じ順序で渡したとき MST root CID が一致すること（決定論性）
- 複数エントリをソートした場合としない場合で同じ root になること（ソート済み前提の検証）
- `encode_car` で生成されたバイト列の先頭が CARv1 ヘッダーバイトで始まること（フォーマット検証）

### 3-4. `ap/client.rs` — HTTP Signature 検証の結合テスト

`build_signing_string` と `parse_signature_header` のユニットテストは充実しているが、実際の RSA 署名 → 検証の一貫した E2E テストがない。RSA 鍵ペアを生成し、`sign_and_post` と同等の署名ロジックで署名した文字列を `verify_signature` で検証するテストを追加する（ネットワーク不要）。

### 3-5. `handlers/notes.rs` — 投稿文字数バリデーションのユニットテスト

`create_note` の文字数バリデーション（バイト数・書記素クラスタ数の 2 段階チェック）はフロントエンドと実質的に同じロジックを持つ。この部分をハンドラから独立した関数に切り出し、境界値（ちょうど 300 graphemes、301 graphemes、絵文字を含む）でテストする。

### 3-6. `id.rs` — スノーフレーク ID の衝突テスト

既存テストでソート可能性・未来補正を確認済み。追加すべきケース:
- 同一ミリ秒内で複数 ID を生成したとき重複しないこと（シリアル値インクリメントの確認）
- 生成した ID が `i64` の正の範囲に収まること（符号ビット確認）

---

## 4. コーディングルール見直し案

### 4-1. ハンドラから Repository を経由しない SQL を禁止するルールの明文化

`CLAUDE.md` またはコードの `repository/mod.rs` の先頭コメントに「ハンドラが `sqlx::query` を直接呼ぶことを禁止。必ず `Arc<dyn XxxRepository>` を経由すること」と明記する。現在コメントに「SQL は各 Pg*Repository の impl 内にのみ記述する」とあるが、`handlers/notes.rs` や `handlers/inbox.rs` で破られている。CI で `rg 'sqlx::query' crates/seiran-api/src crates/seiran-federation-inbox/src` をチェックするスクリプトを置くのも有効。

### 4-2. `eprintln!` 新規追加の抑制

新規コードでは `eprintln!` を使わず `tracing::info!` 等を使う方針を `CLAUDE.md` に追記する。既存の `eprintln!` は段階的に置換。

### 4-3. 外部 I/O を伴う非同期関数のタイムアウト義務化

`fetch_actor`・`sign_and_post` 等の外部 HTTP 呼び出しに対して `tokio::time::timeout` を呼び出し元で必ず設定するルールを定める。現状 `mention.rs` では 2 秒タイムアウトを設けているが、`inbox.rs` の `fetch_actor` 呼び出しはタイムアウトなしで、リモートサーバーの応答遅延がそのままリクエストのブロックになる。
