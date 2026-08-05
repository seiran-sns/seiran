# コード診断レポート（2026-08-05）

対象: `main` の `7d04f37`。前回監査（`code_audit_2026-07-26.md`）以降 119 コミット。
過去レポートで対応済みと確認できた項目は再掲せず、**未解消・新規に判明した事項**のみ記録する。

実測環境: ローカル開発 DB（`seiran-db-1`、posts 64,661 行 / actors 477,511 行 / DB 全体 732 MB）。
数値はすべてこの環境での実測値。

---

## 0. 先に確認した「良好な状態」

診断の前提として、以下は現行コードで実装済みであることを確認した。指摘は行わない。

- ActivityPub inbox の Digest 必須化・ボディ完全性検証・keyId とactor の同一オリジン検証（前回 HIGH-01 の完全対応）
- media proxy の SSRF 対策（IPv4/IPv6 双方の private / link-local / CGNAT / IPv4-mapped 拒否）
- XRPC `uploadBlob` のサービス JWT 検証（`aud` = 自 DID、`lxm` 一致、ES256 固定、`exp` 検証）
- 管理・モデレーション API の認可（`admin/` 配下 29 ハンドラすべてが `require_admin` / `require_emoji_admin` / `require_report_moderator` を通過。`reports.rs` は共通 `authorize()` 経由で 7/7 適用済み）
- パスワードリセットの列挙防止（存在有無で応答を変えない）とトークンのアトミック消費
- SQL は全経路パラメータ化済み。`format!` による SQL 組み立ては `outbox.rs` の 2 箇所のみで、埋め込むのは定数カラムリストであり注入経路なし
- 構造化ログへの移行完了（`println!` / `eprintln!` 残存 0 件）
- CI に gitleaks / fmt / clippy / cargo test / frontend typecheck・lint・vitest / Playwright E2E が揃っている

---

## 1. リファクタリング・ユニットテスト

### [R-1・高] 認可ガードが「ハンドラ本体で呼ぶ」規約に依存しており、型で守られていない

`admin/*.rs` の各ハンドラは本体先頭で `require_admin(...)` を呼ぶ。ルータ側（`lib.rs`）には
`route_layer` による強制がなく、22 本の `/api/admin/*` ルートは**呼び忘れれば無認可で通る**。
現時点で漏れはないが、「新規ハンドラを足したときに 1 行書き忘れる」だけで権限昇格になる構造で、
レビューでしか防げない。

対策: 管理系ルートを `Router::new()...route_layer(middleware::from_fn_with_state(state, require_admin))`
の別ルータへ切り出し、`.merge()` する。ガードを通過した `AuthUser` は `Extension` で渡す。
ハンドラ側の `require_admin` 呼び出しは削除でき、53 ハンドラ分のボイラープレートも消える。

### [R-2・高] タイムライン 4 種で可視性判定 SQL が丸ごとコピペされている

`repository/post.rs` の `home_timeline` / `local_timeline` / `social_timeline`（2 箇所）/
`global_timeline` に、以下 15 行が**5 回**一字一句同じ形で現れる。

```sql
p.visibility NOT IN ('followers_only', 'direct')
OR (p.visibility = 'followers_only' AND (p.actor_id = $1 OR EXISTS (SELECT 1 FROM follows f ...)))
OR (p.visibility = 'direct' AND NOT $5 AND (p.actor_id = $1 OR EXISTS (SELECT 1 FROM post_recipients pr ...)))
```

可視性ルールは今後 list 限定・ローカル限定などで必ず増える。5 箇所同時修正が前提の構造は
必ずどこかが漏れ、しかも漏れた側は「見えてはいけない投稿が見える」形で失敗する。

さらに `local_timeline` / `global_timeline` では、直前に
`AND p.visibility NOT IN ('unlisted', 'followers_only')` があるため、後続の `followers_only`
分岐は**到達不能なデッドコード**になっている。読み手に「ここは followers_only も通ることがある」
と誤読させる。

対策: `actor_is_hidden_for_viewer` と同じ手法で `post_is_visible_to(viewer_id, post_id, exclude_direct)`
を SQL 関数（`STABLE`）として切り出し、5 箇所を 1 行の呼び出しに置き換える。関数化すれば
可視性のユニットテストを SQL レベルで書ける。

### [R-3・中] 巨大ファイルが再び成長している

| ファイル | 行数 | 関数数 |
|---|---|---|
| `jobs/inbound_activity_process.rs` | 2,906 | 44 |
| `handlers/notes/mod.rs` | 2,142 | 25 |
| `ap/deliver.rs` | 2,038 | 38 |
| `atp/service.rs` | 1,807 | (impl 1 個) |
| `frontend/src/api/client.ts` | 1,755 | — |

前回 `notes.rs` を分割した（`notes/{mod,dto,queries,validation,delivery}.rs`）が、`mod.rs` 自身が
再び 2,142 行に戻っている。`inbound_activity_process.rs` は Follow / Create / Announce / Like /
Delete / Flag / Move など**受信アクティビティ全種**が 1 ファイルに同居しており、
`activity_type` ごとのモジュール分割が自然な切れ目になる。

`client.ts` は REST クライアント全域が 1 モジュール。`api/{notes,users,admin,drive,dm}.ts` へ
ドメイン別に割り、`ApiError` / `throwIfError` / `parseJsonBody` を `api/core.ts` に残す形が素直。

### [R-4・中] `Result<_, String>` が 76 箇所残存し、エラー型が二層化している

`ApiError`（HTTP 層）と `ApError`（AP 層）は整備済みだが、ジョブ層は文字列エラーのまま。
`jobs/inbound_activity_process.rs` だけで 15 箇所。`format!("follows INSERT失敗: {}", e)` のような
文字列化はリトライ可否（一時障害 / 恒久失敗）の判定を不可能にし、ジョブキューの再試行戦略を
書けなくしている。

対策: `JobError { Transient(..), Permanent(..) }` を導入し、キューの再試行判断に使う。
一括置換ではなく、リトライ判断が必要なジョブ（配送・外部 API 呼び出し）から順に移す。

### [R-5・中] `atp_blocks` の 1 件ずつ INSERT が 5 箇所に同一コピペ

`atp/service.rs` の L327 / L951 / L1130 / L1285 / L1437 で、以下が完全に同形で 5 回書かれている。

```rust
for (cid, bytes) in &new_blocks {
    sqlx::query("INSERT INTO atp_blocks (cid, actor_id, bytes) VALUES ($1,$2,$3) ON CONFLICT ... DO NOTHING")
        .bind(...).execute(&mut *tx).await?;
}
```

`improvement_db_performance.md` [高-6] で「一括 INSERT に変更」と記載済みだが未着手のまま、
コピー箇所だけが 5 つに増えた。`insert_blocks(&mut tx, actor_id, &new_blocks)` に括り出したうえで
`UNNEST` 一括 INSERT にすれば、1 箇所の修正で 5 経路すべてが速くなる（P-3 と同じ対象）。

### [R-6・中] `sqlx::query!` と `sqlx::query` が混在し、コンパイル時検証が効かない経路がある

`.sqlx/` キャッシュを運用している一方、リポジトリ層の多くは実行時検証の `sqlx::query`
（型は `query_as::<_, T>` で後付け）を使う。カラム追加・型変更がコンパイルで捕まらず、
実行時に初めて落ちる。特に `TimelinePost` のような 20 カラム超の構造体は、
SELECT リストと struct のズレを検出できない。

対策: 新規クエリは `query_as!` を必須とするルールを `coding_rules.md` に追記し、
既存は触った箇所から移行する（一斉移行はコストに見合わない）。

### [R-7・低] CI の clippy が `--all-targets` を付けておらず、テストコードの警告を見逃す

CI は `cargo clippy --workspace -- -D warnings`。ローカルで `--all-targets` を付けて実行すると
`seiran-api` のテストターゲットに 1 件（`items_after_test_module`、`misskey/endpoints.rs`）の
警告が出る。CI をすり抜けている警告が現に存在する。

対策: `cargo clippy --workspace --all-targets -- -D warnings` に変更し、当該 1 件を解消する。

### [R-8・中] テストは量は増えたが、壊れやすい中心部が手薄

現況: Rust 291 件（統合テスト 2 ファイル含む）、frontend 17 ファイル、E2E 20+ spec。
純関数・DTO 変換・バリデーションのカバレッジは良好。一方で以下は**テストが 1 件もない**。

- 可視性判定（R-2 の SQL）— followers_only / direct / block の組み合わせ
- `deliver_to_inboxes` の失敗集計・部分失敗時の戻り値
- `atp/service.rs` のコミットパイプライン（トランザクション境界・MST ルート更新）
- `repository/follow.rs` の状態遷移（pending → accepted → 取り消し）

R-2 で SQL 関数化すれば可視性は DB レベルでテストでき、`crates/seiran-api/tests/support` の
隔離 DB 基盤がそのまま使える。

### [R-9・低] ヘルスチェック / メトリクスのエンドポイントがない

`/health`・`/readyz`・`/metrics` に相当するルートが存在しない。Docker の healthcheck も
DB 以外は未設定で、「API は起動しているが DB プールが枯渇している」状態を外から観測できない。
運用時の障害切り分けが目視ログ頼りになる。

---

## 2. セキュリティ

### [S-1・高] レート制限・アカウントロックがアプリ全体で皆無

`rate limit` / `governor` / `tower_governor` に相当する実装は 1 件も存在しない。影響：

| エンドポイント | 悪用 |
|---|---|
| `POST /api/auth/login` | パスワード総当たり。試行回数制限も遅延もロックもなし |
| `POST /api/auth/register` | 自動アカウント生成（登録は招待制ではない） |
| `POST /api/auth/password-reset` | 任意アドレスへのメール送信増幅（Resend のクォータ・レピュテーション毀損） |
| `POST /api/notes/create` | 投稿スパム。連合先へそのまま配送されるため他インスタンスにも波及 |
| `POST /api/drive/files` | 200 MB ボディ上限のアップロード連打によるストレージ枯渇 |

パスワードは Argon2・最低 8 文字で、TOTP / パスキーも実装済みだが、**試行回数を絞る層がない**
ため 8 文字ポリシーの意味が薄い。

対策: `tower-governor` を導入し、認証系（login / register / password-reset / TOTP 検証）は
IP + 識別子単位で厳しめ（例: 5 回 / 分、以降指数バックオフ）、書き込み系はユーザー単位で
緩め（例: 30 回 / 分）に設定する。ログイン失敗回数は `users` に持たせ、一定回数で一時ロックする。

### [S-2・中] パスワード変更・リセット後も既存 JWT が最大 7 日間有効

`generate_token` は `exp = now + 7 days`、`jti` は発行するが `app_tokens` に記録されるのは
MiAuth 発行分のみ。`middleware/auth.rs` のコメントどおり「記録が無い jti は常に有効」として扱われる。
結果、**通常ログインで発行した JWT はいかなる手段でも失効させられない**。

アカウント乗っ取りに気付いてパスワードを変更しても、攻撃者の手元のトークンは最大 7 日間
有効なままになる。「パスワードを変えれば追い出せる」という利用者の期待に反する。

対策: `users` に `token_valid_after TIMESTAMPTZ` を持たせ、パスワード変更・リセット・
2FA 設定変更時に現在時刻で更新する。`verify_token` 後に `claims.iat < token_valid_after` なら拒否。
DB 参照 1 回で全トークン一括失効が実現できる（jti 個別管理より安い）。

### [S-3・中] CORS が `allow_origin(Any)` + `allow_headers(Any)`

`lib.rs:448`。認証は Cookie ではなく `Authorization` ヘッダーのため古典的な CSRF は成立せず、
`allow_credentials` も付いていない。ただし現状は
「任意のサイトの JS が、ユーザーのブラウザを踏み台に seiran の公開 API を無制限に叩ける」状態で、
S-1（レート制限なし）と組み合わさると、第三者サイトからの分散スクレイピング・
分散総当たりの踏み台にできる。

対策: `FRONTEND_ORIGIN` と自ドメインのみを許可するリストへ変更する。連合・Misskey 互換クライアント
向けの `/api/*` を横断的に開放したい場合でも、`allow_headers` は `authorization, content-type` に
絞れる。

### [S-4・中] 連合用 HTTP クライアントにタイムアウトが設定されていない

`seiran-server/src/main.rs:153,194` の `reqwest::Client::builder()` は `user_agent` のみで
`.timeout()` / `.connect_timeout()` が無い。reqwest の既定はタイムアウト無制限。
`media_proxy` は専用クライアントで 20 秒を設定しているため、**連合経路だけが抜けている**。

悪意ある（あるいは単に壊れた）リモートインスタンスが接続を受理したまま応答しなければ、
配送タスクは永久にハングし、逐次配送（P-3）と組み合わさると後続の全配送が止まる。
Slowloris 型の連合 DoS が成立する。

対策: `connect_timeout(5s)` / `timeout(30s)` を設定する。1 行で塞げる。

### [S-5・中] `npm audit` に moderate 3 件（react-router のオープンリダイレクト）

`GHSA-2j2x-hqr9-3h42`（`@remix-run/router` 1.3.0–1.23.2、`react-router` 6.0.0–7.17.0）。
`//` 始まりのパスがプロトコル相対 URL として再解釈され、同一オリジンリダイレクトが
外部サイトへのリダイレクトになる。前回監査では「SSR 経路のみ」と判断して 6 系を維持したが、
本アドバイザリはクライアント側のリダイレクト処理を対象としている。

なお `RichText.tsx` は `to.startsWith("/") && !to.startsWith("//")` で自前に同種の防御を
入れており、本文由来のリンクからは踏めない。残るリスクは Router 内部のリダイレクト経路。

対策: 7 系（7.18.2 以降）への移行を計画する。破壊的変更を含むため、E2E 20+ spec を回帰の
安全網として使える今のうちが適期。

### [S-6・低] ログイン応答にユーザー列挙の余地がある

`handlers/auth.rs:340-349` は、ユーザーが見つからない時点で `INVALID_CREDENTIALS` を返して
**Argon2 検証をスキップ**する。存在するユーザーでは Argon2 の計算時間（数十〜数百 ms）が加算され、
応答時間の差でアカウントの存在を判定できる。

対策: ユーザー不在時もダミーハッシュに対して `verify` を実行してから同一エラーを返す。

### [S-7・低] PostgreSQL ポートが `0.0.0.0:5432` に公開されたまま（前回 LOW-02 未対応）

`docker-compose.yml:59-60` と `docker-compose.mono.yml:32-33` の双方。前回レポートで指摘済みだが
未修正。ローカル開発では必要だが、同じファイルを本番で使うと DB が外部露出する。

対策: 公開が必要ならバインドを `127.0.0.1:5432:5432` に限定する。本番用 compose では
`ports` を落とし `networks: internal` のみにする。

### [S-8・低] コンテナが root で実行される

`docker/Dockerfile`・`Dockerfile.frontend` のいずれにも `USER` 指定がない。コンテナ内で
任意コード実行に至る脆弱性が出た場合の被害範囲が広がる。

対策: runtime ステージで `useradd -r appuser` し `USER appuser` を指定する。書き込みが必要な
`config/`（secrets.toml）のオーナーを合わせる。

### [S-9・低] CI に依存クレートの脆弱性スキャンがない

gitleaks はあるが `cargo audit` / `cargo deny` がない。Rust 側は RustSec の既知脆弱性を
まったく検知していない状態。frontend も `npm audit` は CI に組み込まれていない。

対策: `cargo audit`（週次 schedule でも可）と `npm audit --audit-level=high` を CI に追加する。

---

## 3. RDB・パフォーマンス

### [P-1・高] `follows` のインデックスが 310 MB に肥大化している（テーブル本体は 96 kB）

```
follows: n_live_tup=427, テーブル 96 kB, インデックス 310 MB
  follows_pkey                                   43 MB  (idx_scan=9)
  follows_follower_actor_id_target_actor_id_key  75 MB  (idx_scan=189)
  idx_follows_follower                           19 MB  (idx_scan=112,065)
  idx_follows_target                             19 MB  (idx_scan=284)
  idx_follows_target_follower                    92 MB  (idx_scan=0)
  idx_follows_follower_accepted                  63 MB  (idx_scan=0)
last_autovacuum: 2026-07-29（以降なし）
```

427 行のテーブルに 310 MB のインデックスが付いている。過去の大量 INSERT/DELETE で B-tree に
空ページが残り、PostgreSQL はこれをテーブルへ返却しないため肥大化したまま固定されている。
フォロワー数 COUNT（P-4 で毎リクエスト走る）はこの肥大インデックスを走査している。

対策: `REINDEX TABLE CONCURRENTLY follows;`。**本番 DB でも同じ状態か必ず確認すること**
（本ローカル DB 固有の事象である可能性はあるが、同じ操作履歴を辿っていれば同様になる）。
恒久対策として、フォロー同期ジョブが大量 DELETE を行う経路を洗い出す。

### [P-2・高] 未使用インデックス 155 MB — 前回レポートで追加したものが使われていない

`idx_follows_target_follower`（92 MB）と `idx_follows_follower_accepted`（63 MB）は
ともに `idx_scan = 0`。`improvement_db_performance.md` [高-3] で「AP 配送のフォロワー取得を高速化」
として追加したものだが、プランナは実際には `idx_follows_follower`（19 MB, 112,065 回）を選んでいる。

書き込みのたびに更新される 155 MB のデッドウェイトになっている。他にも `idx_scan = 0` の
インデックスが posts / media_files / custom_emojis / password_resets などに 13 本ある。

対策: 本番の `pg_stat_user_indexes` を確認したうえで、両方とも DROP する。以後、
インデックス追加は「EXPLAIN で採用されたことを確認してからコミット」を条件にする。

### [P-3・高] AP 配送が完全な逐次処理

`ap/deliver.rs:183` の `deliver_to_inboxes` は `for inbox in inboxes` で 1 件ずつ `.await`。
S-4（タイムアウト無し）と合わせると、応答しないリモート 1 台で配送タスク全体が停止する。
タイムアウトを 30 秒に設定したとしても、フォロワーが 50 サーバーに分散していれば
最悪 25 分を 1 投稿の配送に費やす。

対策: `futures::stream::iter(inboxes).map(...).buffer_unordered(N)`（N = 8〜16 程度）で
並列化する。成功/失敗の集計ロジックはそのまま流用できる。`improvement_db_performance.md` の
「[高] AP 配送の並列化」として計上済みだが未着手。

### [P-4・高] Misskey 互換のフォロー一覧が二重の N+1（1 リクエストで最大 500 クエリ）

`misskey/endpoints.rs:738-754`（`users/following`）と `782-796`（`users/followers`）は、
取得した行ごとに `find_by_id` を呼び、さらに `build_user_detailed` を呼ぶ。
`build_user_detailed`（`misskey/convert.rs:53-100`）は 1 呼び出しにつき

- アバター解決 SELECT × 1
- `SELECT COUNT(*) FROM posts WHERE actor_id=$1 AND deleted_at IS NULL`
- `SELECT COUNT(*) FROM follows WHERE target_actor_id=$1 AND status='accepted'`
- `SELECT COUNT(*) FROM follows WHERE follower_actor_id=$1 AND status='accepted'`

を実行する。`limit` の上限は 100 なので、**1 リクエストで最大 (1 + 4) × 100 = 500 クエリ**。
うち 200 本は P-1 の肥大化した `follows` インデックスを引く COUNT。プール上限は 10 接続（P-7）。

対策:
1. `find_by_id` のループを `find_by_ids(&[i64])`（`WHERE id = ANY($1)`）の一括取得に置き換える
2. カウント 3 種を `actors` の非正規化カラム（`notes_count` / `followers_count` / `following_count`）
   に持たせ、投稿・フォローの増減時に更新する。`posts` には 2026-08-03 に
   `reply_count` / `quote_count` / `repost_count` を追加した実績があり、同じ方式が使える
3. 少なくとも一覧系では `build_user_detailed` ではなく `user_lite` を返す
   （Misskey 本家の `users/following` も lite 相当の情報しか返さない）

### [P-5・中] `actors` が 477,511 行 / 292 MB、うち 473,701 行が投稿を 1 件も持たない

```
actors: bsky 467,603 / fedi 9,777 / local 131
投稿を 1 件も持たないアクター: 473,701（99.2%）
インデックス: search_bigm 36 MB, at_did 34 MB, username_domain 26 MB, handle_prefix 20 MB …
last_autovacuum: なし（n_tup_upd 58,264、n_dead_tup 7,569）
```

Bsky 側から流入したアクターを、自インスタンスと関係の有無にかかわらず保存し続けている。
DB 全体 732 MB のうち 292 MB がこれで、ユーザー検索（`idx_actors_search_bigm`）の
対象行数を 47 万件に膨らませている。ローカルユーザーは 131 人。

autovacuum が一度も走っていない点も要注意（`autovacuum_vacuum_scale_factor = 0.2` の既定値では
47 万行に対し約 9.5 万行の更新が必要で、更新 5.8 万回ではまだ閾値に届かない）。
このまま増えると VACUUM 一回あたりの負荷が跳ね上がる。

対策:
1. Bsky アクターの保存条件を見直す（投稿・フォロー・言及のいずれかで自インスタンスと
   関わったアクターのみ永続化し、それ以外は保存しないか TTL 付きで掃除する）
2. `actors` に `ALTER TABLE actors SET (autovacuum_vacuum_scale_factor = 0.02)` を設定する
3. 検索インデックスを `WHERE actor_type = 'local' OR <関与あり>` の部分インデックスにする

### [P-6・中] `atp_repo_events` が 3,340 行で 42 MB（1 行平均 12 KB）

CAR バイト列をそのまま行に格納しているため。`improvement_db_performance.md`
「[低] `atp_repo_events` の car_bytes 外部ストレージ」として計上済み。現時点で 42 MB なら
緊急ではないが、投稿量に比例して線形に増え、`pg_dump` / レプリケーションの重量物になる。

対策: 一定期間（Relay の再取得要求が来なくなる期間、実運用では 72 時間程度）を過ぎた
`car_bytes` を NULL 化する定期ジョブを入れる。イベントのメタデータは残す。

### [P-7・中] 絵文字データが言語ごとに丸ごと別チャンク（745〜808 kB × 7）

production build の実測:

```
index-*.js              583 kB (gzip 180 kB)
data-*.js  × 7          745〜808 kB (gzip 90〜107 kB) ← emojibase-data の言語別データ
EmojiPickerPanel-*.js   281 kB (gzip 32 kB)
Panel.module-*.js       165 kB (gzip 59 kB)
```

絵文字ピッカーを初めて開いた時点で、その言語の `data-*.js`（gzip 約 100 kB）+
`EmojiPickerPanel`（gzip 32 kB）を取得する。lazy 分割自体は効いているが、
1 言語分のフルデータをまとめて落とす構成のため、モバイル回線では体感できる待ちになる。
また main chunk は前回計測の 570 kB から 583 kB へ微増している。

対策: `emojibase-data` はショートコード解決にしか使っていないなら、必要なフィールドだけを
ビルド時に抽出した軽量 JSON を生成する（`scripts/copy-twemoji-assets.mjs` と同じ postinstall 方式）。
グループ単位の遅延読み込みでもよい。

### [P-8・低] DB プールが `max_connections(10)` ハードコード、`acquire_timeout` 未設定

`common/src/db.rs:12`。環境変数で調整できず、split-role 構成（api / federation / worker）では
プロセスごとに 10 で合計 30 になる。`acquire_timeout` は sqlx 既定の 30 秒のままで、
プール枯渇時にリクエストが 30 秒ハングしてから 500 を返す。P-4 の N+1 と同居すると、
フォロワー一覧 3 リクエストでプールを占有しうる。

対策: `DB_MAX_CONNECTIONS` で設定可能にし、`acquire_timeout` を 5 秒程度に縮める
（速く失敗させて 503 を返すほうが観測しやすい）。

### [P-9・低] 計測基盤がない

`pg_stat_statements` が未導入（`SELECT count(*) FROM pg_extension` = 0）。
アプリ側にもメトリクス出力（R-9）がない。前回監査で「endpoint 別の p95 / DB query 時間を
計測してから TTL を決める」としてキャッシュ導入を見送っているが、**その計測手段自体が
まだ用意されていない**ため、判断が先送りされ続ける構造になっている。

対策: `shared_preload_libraries = 'pg_stat_statements'` を `docker/Dockerfile.postgres` の
設定に追加する。これが入れば P-1〜P-4 の効果も定量的に確認できる。

### [P-10・低] バックグラウンドジョブのループ内 SELECT

`jobs/actor_history_sync.rs:89,205` は取得した投稿 1 件ごとに
`SELECT id FROM posts WHERE ap_object_id = $1 LIMIT 1` で重複チェックする。
バックフィル用途で即時性の要求はないが、`ap_object_id = ANY($1)` の一括問い合わせに
まとめれば 1 クエリで済む。

---

## 4. 優先順位の提案

即応（1 日以内で効果が出るもの）:

1. **S-4** 連合クライアントに timeout を設定（1 行、連合 DoS を塞ぐ）
2. **P-2** 未使用インデックス 155 MB を DROP（本番の統計確認後）
3. **P-1** `REINDEX CONCURRENTLY follows`（本番の状態確認後）
4. **S-7** DB ポートのバインドを `127.0.0.1` に限定

短期（1 スプリント）:

5. **S-1** レート制限の導入（認証系優先）
6. **P-3** AP 配送の並列化
7. **P-4** フォロー一覧の N+1 解消
8. **S-2** パスワード変更時のトークン一括失効
9. **R-1** 管理ルートの認可をルータ層へ移す

中期:

10. **R-2** 可視性 SQL の関数化 + テスト
11. **P-5** Bsky アクター保存方針の見直し
12. **S-5** react-router 7 系移行
13. **R-3** 巨大ファイルの再分割
14. **P-9 / R-9** 計測基盤（pg_stat_statements・/metrics）

---

*本レポートは 2026-08-05 時点の `7d04f37` を対象とし、実測はローカル開発 DB による。*
*DB 関連の指摘（P-1 / P-2 / P-5）は本番環境の統計値を確認したうえで適用すること。*
