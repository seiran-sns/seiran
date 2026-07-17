# Doc 6. 開発作業手順書 ＆ マイルストーン・ロードマップ (Development Roadmap & Checklist)

本ドキュメントは、`seiran` システムの実装における具体的な作業手順と、進捗管理用のチェックリストを定義する。
開発の各フェーズにおいて、完了した項目にチェック `[x]` を入れて進捗を管理すること。

---

## 📅 フェーズ1: 開発環境セットアップ ＆ データベース移行 (DB Schema)
データベースのマイグレーションと、システムの中核となる統一ポストID (Snowflake) 採番エンジンを実装する。

- [x] **1.1. データベース・マイグレーション・スクリプトの作成**
  - [x] `users` テーブル（ローカル認証）の実装
  - [x] `actors` テーブル（統一アクター情報、`actor_type_enum`、ペアリング/ブリッジ参照ポインタ）の実装
  - [x] `posts` テーブル（統一ポスト、リレーション、重複マージ用UUID/ポインタ、プロトコル固有ID）の実装
  - [x] 各インデックス（`idx_actors_type`, `idx_actors_pair`, `idx_actors_bridge`, `idx_posts_snowflake` 等）の設定
  - [x] DBパフォーマンスインデックス追加（`idx_actors_user_id`, `idx_actors_username_domain`, `idx_follows_target_follower`, `idx_posts_actor_id` 複合部分インデックス化）
  - [x] ローカルタイムライン高速化: `posts.is_local` 非正規化カラム + トリガー + `idx_posts_local_active` 部分インデックス、`idx_follows_follower_accepted` カバリングインデックス追加（2026-07-10、詳細は `docs/improvement_db_performance.md` 追記分参照）
- [x] **1.2. 統一ID (Snowflake / ULID) 採番エンジンの実装**
  - [x] 48bit タイムスタンプ ＋ 16bit シリアルノードID/ランダム値による一意ID生成モジュールの実装
  - [x] 未来補正タイムスタンプアルゴリズムの実装:
    `if post.created_at > SYSTEM_NOW() { id_timestamp = SYSTEM_NOW(); } else { id_timestamp = post.created_at; }`
- [x] **1.3. データベース接続レイヤー (Rust/SQLx等) のセットアップ**
  - [x] Connection Pool設計、接続テストの実行

---

## 🔒 フェーズ2: プラガブル認証システム ＆ MiAuth互換 (Auth Layer)
ローカル ID/PW 認証のみを使用（Auth0 は廃止）。Misskeyクライアント向けのMiAuthエンドポイントを実装する。

- [x] **2.1. ローカル認証プロバイダ実装**
  - [x] `users` テーブルとの連携、Argon2 によるパスワードハッシュ、JWT 発行
  - [x] `users.auth0_sub` カラム削除（Auth0 廃止）
- [x] **2.2. MiAuth (Misskey認証) 互換エンドポイントの実装**
  - [x] `/miauth/authorize` エンドポイントの実装
  - [x] ローカル JWT と紐付けたアクセストークン発行ロジックの実装
- [x] **2.3. メールアドレス確認フロー（Email Verification）**
  - [x] `email_verifications` テーブルのマイグレーション追加（`id, email, token UUID, expires_at, verified_at, created_at`）
  - [x] `POST /api/auth/verify-email` — メールアドレスを受け取り確認トークンを生成・送信
  - [x] `GET /auth/verify?token=...` — トークン検証・`verified_at` を記録しクライアントへトークンを返却
  - [x] `POST /api/auth/register` に `registration_token` 必須検証を追加（未確認メールでの登録を拒否・レコードを DELETE して使い捨て）
  - [x] `lettre` クレートによる SMTP 送信実装（`seiran-api/src/mailer.rs`）
  - [x] `.env` に SMTP 設定追加（Resend / smtp.resend.com / info@seiran.org）
  - [x] `.env.example` に SMTP 設定例を追加
- [x] **2.4. Aria (Misskey クライアント) MiAuth 互換対応**
  - [x] `POST /api/meta` エンドポイント実装（`{"features": {"miauth": true, "registration": true}, "uri": "...", "name": "seiran", "version": "..."}` を返す）
  - [x] MiAuth check エンドポイント追加: `/api/miauth/:session_id/check`（パスでセッション受け取り、ボディなし）← Aria が期待する形式
  - [x] 旧 `/api/miauth/check`（ボディ形式）は後方互換で残存
  - 仕様詳細: `docs/03_multi_protocol_engine_specification.md` セクション 5 参照
- [x] **2.6. 構造化 API エラーレスポンス（i18n 基盤）**
  - [x] `ApiError` が `{"code": "ERROR_CODE"}` の JSON を返すように変更（従来の平文テキストから移行）
  - [x] エラーコード体系を定義（`EMAIL_ALREADY_REGISTERED`, `USERNAME_TAKEN`, `INVALID_CREDENTIALS` 等）
  - [x] 登録済みメールアドレスを `request_email_verification` で事前検出し `EMAIL_ALREADY_REGISTERED` を返す
  - [x] フロントエンド `client.ts` に `ApiError` クラスと `getErrorMessage()` 関数を追加（コード→日本語メッセージのマップ）
  - [x] `Register.tsx` / `VerifyEmail.tsx` でエラーメッセージ表示を `getErrorMessage()` 経由に統一
  - [x] 方針を `docs/02_architecture_and_overall_design.md` セクション 1.4 に文書化
- [x] **2.7. ユーザーネームによるログイン対応**
  - [x] `POST /api/auth/login` の `email` フィールドを `identifier` に変更（後方互換なし）
  - [x] `identifier` に `@` が含まれる場合はメールアドレス、含まれない場合はユーザーネームとして解決
  - [x] ユーザーネーム解決: `actors WHERE username = $1 AND domain = LOCAL_DOMAIN` で `user_id` を取得
  - [x] フロントエンド `Login.tsx` のフィールドラベルを「メールアドレス / ユーザーネーム」に変更
  - [x] `client.ts` の `login()` の引数名を `identifier` に変更
  - 仕様詳細: `docs/02_architecture_and_overall_design.md` セクション 1.5 参照
- [x] **2.8. パスワードリセット機能**
  - [x] マイグレーション: `password_resets` テーブル（`id, user_id, token UUID, expires_at 1h, created_at`）
  - [x] `POST /api/auth/request-password-reset` — メールアドレスを受け取りリセットリンクを送信（ユーザー不在でも同一レスポンス）
  - [x] `GET /api/auth/verify-reset-token?token=...` — トークン検証（副作用なし）
  - [x] `POST /api/auth/reset-password` — `{ token, new_password }` でパスワード更新・トークン消費
  - [x] フロントエンド: `/forgot-password` ページ（メール入力）、`/reset-password?token=...` ページ（新パスワード設定）
  - [x] エラーコード追加: `RESET_TOKEN_INVALID`, `PASSWORD_TOO_SHORT`
  - 仕様詳細: `docs/02_architecture_and_overall_design.md` セクション 1.6 参照
- [ ] **2.9. Turnstile 自然人判別（優先度: 低）**
  - [ ] `TURNSTILE_SECRET_KEY` 環境変数が設定されている場合のみ検証を有効化（未設定時はスキップ）
  - [ ] バックエンド: `POST /api/auth/verify-email`, `login`, `request-password-reset` でトークン検証
  - [ ] フロントエンド: `TURNSTILE_SITE_KEY` を持ち込んだ場合に Turnstile ウィジェットを表示
  - [ ] `.env.example` に `TURNSTILE_SITE_KEY` / `TURNSTILE_SECRET_KEY` プレースホルダーを追加
  - 仕様詳細: `docs/02_architecture_and_overall_design.md` セクション 1.7 参照
- [x] **2.5. シークレット自動生成・永続化 (`secrets.toml`)**
  - [x] `seiran-common::secrets` モジュールの実装
  - [x] `JWT_SECRET`（256bit hex）を起動時に `OsRng` でランダム生成し `config/secrets.toml` に保存
  - [x] AT Protocol PDS 用 P-256 鍵ペア（秘密鍵・公開鍵 PEM）を同ファイルに自動生成
  - [x] `SEIRAN_CONFIG_DIR` 環境変数でディレクトリ変更可能（デフォルト: `./config`）
  - [x] ファイルパーミッション `0600` 設定、`config/` を `.gitignore` に追加
  - [x] 環境変数 `JWT_SECRET` を廃止し `.env.example` から削除

---

## ⚙️ フェーズ3: プラガブル非同期ジョブキュー ＆ 物理分割準備 (Job Queue)
オンメモリとRedisを切り替え可能にし、システムを疎結合に保つための非同期ジョブキューの共通基盤と各ジョブハンドラを実装する。

- [x] **3.1. `JobQueue` Trait (インターフェース) の定義**
  - [x] ジョブのプッシュ、ポップ、リトライ、キャンセル等のメソッド定義
- [x] **3.2. キュー実装の切り替え対応**
  - [x] `InMemoryJobQueue` の実装（モノリスモード `--role all` 用、オンメモリ）
  - [x] `RedisJobQueue` の実装（split-role構成用。優先度付き Sorted Set + `BZPOPMIN` + Lua スクリプトによる遅延リトライ昇格。2026-07リファクタリングで実装）
  - [x] ロールに応じたバックエンド自動選択（`seiran_common::create_job_queue`）: `all` は常に InMemory、split-role は `REDIS_URL` の有無で Redis/InMemory を切替
  - [x] `docker-compose.yml`（大規模構成）に `redis` サービスを追加。`docker-compose.mono.yml` には追加しない
  - [x] `WorkerEngine` をバックエンド非依存化（`Arc<dyn JobQueue>`）し、同時実行数の上限（既定32）を追加
  - [x] リトライを `JobQueue::enqueue_retry` によるキューへの再投入に変更（Redis利用時はプロセス再起動を跨いでリトライ状態が残る）
- [x] **3.3. 5大非同期ジョブハンドラの実装**
  - [x] **① 過去ログ同期キュー (`actor_history_sync`)**
    - [x] ドメイン単位 of 同時実行制限（Concurrency Limit）の適用
    - [x] 1〜3秒のジッター挿入、指数バックオフで最大3回リトライ
  - [x] **② AP 配送キュー (`ap_delivery`、旧 `outbound_post_delivery`)**
    - [x] 高優先度処理、相手サーバーダウン時の長期指数バックオフ（最大10回リトライ）
    - [x] `ApDeliveryKind`（Create/Announce/Undo/Update/Delete/リアクション）として実装し、API ハンドラの `tokio::spawn` 直接配送を enqueue に置換（2026-07 リファクタリング）
  - [x] **③ 配送受け入れ（インバウンド）キュー (`inbound_activity_process`)** — 実装済み（2026-07リファクタリング）。federation-inbox の `inbox_handler` は署名検証のみ同期実行し、Follow/Create/Accept/Undo/Announce/Like・EmojiReact 全種別をこのキューへ委譲する。ハンドラ本体は `seiran-common::jobs::inbound_activity_process` に移設し、`JobContext.inbox`（`InboxContext`）経由でリポジトリ・`stream_hub`・AP秘密鍵にアクセスする
    - [x] 指数バックオフ最大3回リトライ
    - [ ] ドメイン単位のレート制限（未実装、今後の課題）
  - [ ] **④ アクター検証・メタデータ取得キュー (`actor_metadata_resolve`)** — 未実装（ゼロトラストペアリング統合時に実装）
    - [ ] `/verify-actor` ハンドシェイク検証、Webfinger解決、アバター画像等のキャッシュ
  - [x] **⑤ ATPリポジトリコミット・配信キュー (`atp_repository_publish`)**
    - [x] 極高優先度、アクターID単位 of FIFO（先入れ先出し）制御・排他ロックの適用
- [x] **3.4. 統合バイナリ化（単一バイナリ・複数ロール）**
  - [x] 各ロールクレート（api / inbox / worker / atp-repo）を lib 化（`main` を廃止し `init_state` / `router` / `run` を公開）
  - [x] 統合バイナリ `seiran-server` 新設（`--role` / `SEIRAN_ROLE` で分岐、引数なしは `all`）
  - [x] `all` モードで api + federation を `Router::merge`、worker / firehose を spawn、共有リソース（DB プール・シークレット・HTTP クライアント）はプロセス内で一度だけ生成
  - [x] Dockerfile を単一バイナリビルドに集約（マルチターゲット廃止）
  - [x] `docker-compose.yml`（大規模・ロール分離）と `docker-compose.mono.yml`（小規模・単一コンテナ）を用意
  - [x] `docker/nginx.conf`（split）と `docker/nginx.mono.conf`（mono）を用意
  - 仕様詳細: `docs/02_architecture_and_overall_design.md` セクション 4.0 参照

---

## 🌐 フェーズ4: マルチプロトコル通信エンジン ＆ ゼロトラストペアリング (Federation Engine)
ActivityPubおよびAT Protocolのプロトコルレベルの同期・解決、そしてリモートseiranユーザー同士の直接ペアリングを実装する。

- [x] **4.1. ActivityPub (Fediverse) 統合**
  - [x] Webfinger解決、Inbox（受取）および Outbox（過去ログ配送）ハンドラの実装
  - [x] HTTP Signaturesによる署名検証、公開鍵キャッシュ
  - [x] Outbox同期時の「過去30日間 / 最大300件」キャップ処理 (ベストエフォート)
- [x] **4.2. AT Protocol (Bluesky) 統合**
  - [x] DID解決（`did:plc`, `did:web`）、AppView APIクライアントの実装
  - [x] `getAuthorFeed` を用いた外部アクターの過去ログフェッチ（過去30日間 / 最大300件キャップ）
  - [x] Bluesky Firehose 受信モジュールの実装（外部アクターの新着ポストをリアルタイム DB 保存）
  - [x] seiran PDS としてのローカルユーザー ATP リポジトリ管理（MST署名コミット + Relay 配信）
    - [x] ユーザー登録時の `did:plc` 発行（plc.directory への登録）
    - [x] 投稿時の DAG-CBOR エンコード + MST コミット + P-256 署名
    - [x] `com.atproto.sync.getRepo` / `getRecord` / `subscribeRepos` エンドポイント実装
    - [x] `subscribeRepos` コミットイベントの Redis Pub/Sub プロセス間配信ブリッジ（`api` ロールの水平スケール時、`REDIS_URL` 設定時のみ有効。`AtpCommitService::with_redis_bridge`。2026-07リファクタリングで実装）
    - [x] `com.atproto.server.describeServer` エンドポイント実装（Relay の PDS 検証用）
    - [x] `/.well-known/did.json` エンドポイント実装（サーバー DID `did:web:{domain}` 提供）
    - [x] 初回コミット時に `com.atproto.sync.requestCrawl` で Relay (`bsky.network`) へ通知（200 OK 確認済み）
    - [x] ユーザー登録時に `app.bsky.actor.profile/self` をコミット（AppView へのアクター認識に必須）
    - [x] `atp_records` テーブル追加（profile 等 non-post レコードを管理し MST 再構築に使用）
    - [x] `com.atproto.identity.resolveHandle` エンドポイント実装（Relay のハンドル検証に必須・未実装だと handle.invalid になる）
    - [x] Cloudflare DNS TXT によるハンドル検証（`_atproto.{handle}` TXT レコード自動管理）
    - [x] DID確定 → TXT セット → PLC送信 の順序保証（plc.directory イベント配信前に TXT を配置）
    - [x] PLC 送信リトライ時の genesis 再生成（同一署名での連続失敗を防止）
    - [x] `commit_record_inner` DB 操作 4 本をトランザクション化（atp_blocks INSERT・actors UPDATE・atp_records INSERT・atp_repo_events INSERT をひとつのトランザクションに束ね、WebSocket ブロードキャストを commit 後に実行）
    - [x] `atp_repo_events.frame_bytes` カラム追加（zstd 圧縮）: commit 時生成フレームを保存し cursor 再送で再構築せずそのまま送出（再構築フレームと commit 時フレームのバイト列差異による Relay 切断を解消）
    - [x] `#identity` フレーム実装（`atp_repo_events.event_type` 列追加で commit/identity を同テーブル管理）: ユーザー登録完了後に Relay へ handle 再検証を促す。起動時に identity イベント未送信の既存ユーザーへ自動補完。
    - [x] 画像 embed の DAG-CBOR 形式修正: blob ref は `{"$link": tag42}` ではなく `Ipld::Link(cid)` = tag42 直接（AT Protocol CBOR 仕様準拠）
    - [x] `#commit` フレームの `blobs` フィールド実装: 新規コミットで参照する blob CID リストを含める（空だと AppView が画像投稿をインデックスしない）
    - [x] `com.atproto.sync.getBlob` 実装: sha256 で `media_files` を検索し CDN URL へ 307 リダイレクト
    - [x] **fix: Bsky配送不安定性の主因2件を修正**（2026-07-17、indigo/cmd/relay をローカル起動し実地検証して特定。Doc3 §10.6）: ①`#commit` フレームの `prevData` フィールド欠落により Sync 1.1 検証で2回目以降の全コミットが `missing prevData field` でリジェクトされていた（`actors.at_repo_data_cid` カラム追加で前回 MST root CID を保持し次回コミットへ渡すよう修正）。②ECDSA署名を low-S に正規化していなかったため約50%の確率で `cryptographic signature invalid` としてランダムに拒否されていた（`create_commit`・PLC genesis operation 署名の両方に `normalize_s()` を追加）。
    - [x] **fix: 動画埋め込み(`app.bsky.embed.video`)がBsky公式アプリで再生できない不具合を修正**（2026-07-17、マイケル実機確認・再現。Doc3 §12.3/§12.4）: ①`sign_service_auth_jwt`（`jsonwebtoken`/`ring`バックエンド）もlow-S未正規化で約50%の確率でBsky動画パイプラインへの認証が失敗していた（署名セグメントのみ取り出しnormalize_sで矯正）。②`uploadBlob`受け口が受信バイト列を意図的に読み捨てており、`video.bsky.app`が代理POSTしてくるトランスコード済みバイナリが保存されず`getBlob`が404、動画が再生不能だった（新設`atp_blobs`テーブルに保存するよう修正。Doc1 §1.15）。③Content-Typeヘッダーが`*/*`で来ることがあるためマジックバイトからのMIME sniffに変更。④投稿ボタンを押すのが早いと`bsky_video_status`未確定のまま`external`フォールバックに固定される問題を`Job::BskyPostCommitDeferred`で解消（動画パイプライン結合完了を待ってからBskyコミット、固定3秒×最大20回リトライ+70秒タイムアウトフォールバック）。⑤`atp_blobs`の安全性強化: pending動画ジョブへの紐付け必須化（無制限アップロード防止）・`media_files`とのクロステーブル重複排除・7日GC追加。
    - [x] **fix: `com.atproto.repo.getRecord`がembed(画像・動画・引用等)を一切返さないバグを修正**（2026-07-17、マイケル指摘で発覚。Doc3 §10.7）: CIDリンク（DAG-CBOR tag 42）を含むレコードを`serde_json::Value`へ直接デシリアライズすると`invalid type: newtype struct`で失敗しており、投稿本文とcreatedAtだけからその場でJSON再構築するフォールバックしか動いていなかった。`Ipld`経由でAT Protocol標準のJSON表現（`$link`/`$bytes`）に変換するヘルパー`ipld_to_json`を追加して解消。
    - [x] **feat: 音声・動画フォールバックの`app.bsky.embed.external`に簡易視聴ページを追加**（2026-07-17、マイケル指摘。Doc3 §12.5）: Bskyには音声専用embedが無く、動画もパイプライン未完了時はexternalフォールバックになるが、そのリンク先がメディアファイルの直リンクだとダウンロードになり再生できなかった。`GET /api/media/:media_file_id/watch`（`handlers::drive::watch_media`）で`<video>`/`<audio>`タグ1個だけの視聴ページを返すようにし、`commit_post`のフォールバックURLをそちらに向けた。
  - [x] Bsky 向けメンション変換の基礎実装（ローカル `@user` → `@user.{domain}`、Fedi リモート → brid.gy 2段階ルックアップ）
  - [x] Bsky 向けメンション Facet 生成（AT Protocol RichText facets への変換。現状はテキスト置換のみ）
  - [x] **リポスト重複制約**: `UNIQUE INDEX (actor_id, repost_of_post_id) WHERE deleted_at IS NULL` をマイグレーションで追加（取り消し前の再リポストを DB レベルで禁止）
  - [x] **リポストのクロスプロトコル配信**（spec doc 9.1〜9.3 節参照）
    - [x] Fedi リモートポストのリポスト → Bsky: DB の at_uri 確認 → フォールバック URL テキスト投稿（初期実装: ブリッジ探索なし）。フォールバック投稿はリポストラッパー行自身を PDS テキストポストとして commit し、取り消し時は retract する（別行の重複ノートを作らない）
    - [x] Bsky リモートポストのリポスト → Fedi: DB の ap_object_id 確認 → フォールバック URL テキスト投稿（初期実装: ブリッジ探索なし）
    - [x] ローカル / リモート seiran ポストのリポスト → 両方に配信
  - [x] **引用投稿のクロスプロトコル配信**（spec doc 9.4 節参照）
    - [x] Fedi 引用 → Bsky: `app.bsky.embed.external`（元ポスト URL カード）で代替
    - [x] Bsky 引用 → Fedi: quoteUrl として bsky.app URL を付与
  - [x] **リプライの配信先制御**（spec doc 9.5 節参照）
    - [x] 元ポストが Fedi リモートならリプライを Fedi のみに配信（Bsky 配信しない）
    - [x] 元ポストが Bsky リモートならリプライを Bsky のみに配信（Fedi 配信しない）
    - [x] ローカル / リモート seiran ポストへのリプライは両方に配信
- [x] **4.3. ActivityPub 双方向フェデレーション完成**
  - [x] WebFinger エンドポイント（`GET /.well-known/webfinger`）をDBと接続し実データを返す（Content-Type: application/jrd+json 固定）
  - [x] Actor ドキュメント（`GET /users/:username`）をDBと接続し実データを返す
  - [x] NodeInfo エンドポイント（`GET /.well-known/nodeinfo` / `GET /nodeinfo/2.1`）の実装
  - [x] Inbox 受信処理: `Follow` / `Accept` / `Undo(Follow)` / `Create(Note)` のインライン処理実装
  - [x] フォロー関係 DB テーブル（`follows`）の `status` カラム追加（`pending` / `accepted`）
  - [x] こちら発のフォロー（`POST /api/follows/create`）: WebFinger → Actor取得 → Follow Activity送信 → pending状態で記録
  - [x] 投稿配送: 自アクターへの Follow 受理後、新規投稿を相手 Inbox へ HTTP Signatures 付きで配送
  - [x] Outbox エンドポイント（`GET /users/:username/outbox`）の実装
  - [x] AP 向け ATP ハンドル変換の基礎実装（`@handle.tld` → brid.gy WebFinger 2段階ルックアップ → AP メンション or Markdown リンク）
- [ ] **4.4. リモート seiran アクター専用ハンドシェイク (ゼロトラスト検証)**
  - [ ] Bioの `seiran_signature: [ATP_DID]` パターン検出ロジックの実装
  - [ ] 相手ドメインの `/.well-known/seiran/verify-actor` への検証リクエストの実装
  - [ ] 検証成功時の `actor_type = 'remote_seiran'` 昇格と `seiran_pair_actor_id` の相互紐付け
- [ ] **4.5. リモート seiran 特権初期同期**
  - [ ] 特権同期エンドポイント `/api/seiran/v1/posts/export` の実装
  - [ ] 相手サーバーから生データ（body ＋ metadata）を最大300件バルク取得・直接インポートする処理

---

## 🖥️ フェーズ4.5: フロントエンドMVP ＆ ローカル機能 (Frontend MVP)
動作確認を早期に実現するため、フロントエンドとローカル機能を優先実装する。
フレームワーク: **React + Vite + TypeScript**（Vue.js から変更）

- [x] **4.5.1. ローカル機能 API の実装 (seiran-api)**
  - [x] `POST /api/auth/register` — ユーザー登録（Argon2ハッシュ、users + actors テーブル保存）
  - [x] `POST /api/auth/login` — ログイン（パスワード検証、JWT発行）
  - [x] `GET /api/auth/me` — 自分のユーザー情報取得（JWT認証）
  - [x] `POST /api/notes/create` — ローカル投稿作成（AP配送・ATP コミット連動）
  - [x] `GET /api/notes/local-timeline` — ローカルタイムライン（`sinceId` / `untilId` ページネーション）
  - [x] `GET /api/notes/home-timeline` — ホームタイムライン（フォロー中のリモートアクター投稿含む）
  - [x] `POST /api/follows/create` — フォロー送信（Fedi AP / Bsky ATP / ローカル 統合）
  - [x] `POST /api/follows/delete` — フォロー解除（AP Undo Follow / ATP コミット削除 / ローカル）
  - [x] `GET /api/users/profile` — ユーザープロフィール取得（ローカル / リモート統合）
  - [x] `GET /api/notes/:id` — ポスト詳細取得（フロントエンド向け JSON）
  - [x] `GET /notes/:id` — ActivityPub Note エンドポイント（Fediverse 向け、ローカルポストのみ）
    - nginx `map $http_accept` でコンテンツネゴシエーション（AP → api、ブラウザ → frontend）
  - [x] 投稿配信先の Fedi/Bsky 独立トグル（`deliver_to_fedi` / `deliver_to_bsky`）
  - [x] 配信先に応じた文字数制限連動（Bsky: 3,000B/300grapheme、Fedi/Local: 10,000B/3,000grapheme）
- [x] **4.5.2. フロントエンド (React + Vite + TypeScript)**
  - [x] `frontend/` プロジェクト初期化
  - [x] ログイン画面 ・ ユーザー登録画面
  - [x] ローカルタイムライン画面（ローカル / ホーム タブ切り替え）
  - [x] ホーム / ローカル / リストタイムラインの無限スクロール（`until_id`カーソル + `IntersectionObserver`、既存のバックエンドAPIはページネーション対応済みだったためフロント実装のみで対応。詳細はDoc3 §5.9）
  - [x] プロフィール画面の投稿一覧の無限スクロール（新規 `GET /api/users/posts` エンドポイント + `PostRepository::timeline_by_actor` のカーソル対応拡張、`ProfileResponse.actor_id` 追加。詳細はDoc3 §5.9）
  - [x] 投稿入力フォーム
  - [x] ユーザープロフィール画面（`/profile?q=...`）: ローカル・リモートユーザー表示、フォローボタン・フォロー解除ボタン（ローカル / Fedi / Bsky 統合）
  - [x] タイムライン上のユーザー名クリック → プロフィール画面遷移
  - [x] ポスト詳細画面（`/notes/:id`）: 単一投稿表示、AP エントリーポイント兼用
  - [x] `GET /api/notes/:id/context` — ノート詳細コンテキスト（前後投稿各10件）、リモートアクター未フォロー時は AP Outbox から同期フェッチ
  - [x] ポスト詳細画面に前後ノートコンテキスト表示（前後各10件、クリックで遷移）
  - [x] 文字数オーバーエラー（`TEXT_TOO_LONG`）のフロントエンド表示
  - [x] タイムライン・ポスト詳細・ユーザー詳細で共通のポストカード（`NoteCard`）を使用（#43）。`ProfileResponse.recent_posts` を `NoteResponse` 形で返却

---

## 🖼️ フェーズ4.6: メディアアップロード ＆ 管理機能 (Media & Admin)

画像アップロード基盤・オブジェクトストレージ統合・ユーザーロール・管理画面を実装する。

### DB マイグレーション
- [x] `user_role` ENUM 型（`user` / `moderator` / `admin`）を追加し `users.role` カラムを追加
- [x] `users.suspended_at TIMESTAMPTZ` カラムを追加
- [x] `actors.avatar_media_id` / `actors.banner_media_id` カラムを追加
- [x] `storage_providers` テーブル新規作成（スキーマ: `docs/01_database_schema_blueprint.md` 1.7）
- [x] `media_files` テーブル新規作成（スキーマ: 1.8）
- [x] `post_attachments` テーブル新規作成（スキーマ: 1.9）
- [x] `custom_emojis` テーブル新規作成（スキーマ: 1.10）

### secrets.toml
- [x] `encryption_key`（256bit、AES-256-GCM 用）を自動生成・永続化

### オブジェクトストレージ統合
- [x] `storage_providers` CRUD（管理者 API）
  - `secret_key` は `encryption_key` で AES-256-GCM 暗号化して格納
  - `GET/POST /api/admin/storage-providers`、`PATCH/DELETE /api/admin/storage-providers/:id`
  - `require_admin` ミドルウェア（JWT → DB ロールチェック）実装済み
- [x] ストレージ選択アルゴリズム実装（id 順・`capacity_mb` チェック）
- [x] S3 互換 PUT / DELETE クライアント実装（`aws-sdk-s3`）

### 画像処理パイプライン
- [x] WebP 変換・リサイズ処理実装（`image` クレート）
  - アバター: 中央正方形クロップ → 600 × 600
  - バナー: fit inside → 横最大 2048 × 縦最大 768
  - カスタム絵文字: fit inside → 横最大 384 × 縦最大 64
  - ポスト添付: fit inside → 長辺最大 2048
- [x] SHA-256 + blurhash 計算（WebP 変換後に算出）
- [x] `(sha256, blurhash)` 複合一致による重複排除
- [x] クォータチェック実装（ポスト送信・プロフィール保存時に実参照から計算）
- [x] WebP 変換後サイズが元画像より大きい場合は元形式（jpeg/png）のまま保存する
  - `media_files.sha256` は「実際に保存したバイナリ」のハッシュ値を持つ（WebP なら WebP のハッシュ、元画像なら元画像のハッシュ）
  - `media_files.mime_type` は保存形式に合わせて更新（WebP でなければ元の MIME type を保持）
  - 元形式保存時も `image` クレートで再エンコードして Exif（GPS タグ等）を除去

### アップロード API
- [x] `POST /api/drive/files/create` — 画像アップロード（Misskey 互換）
  - オブジェクトストレージ未設定時は `503` を返す
  - ドライブ UI はフロントエンドに公開しない
- [x] ポスト添付: `post_attachments` に保存（上限 10 枚。Bsky embed は先頭 4 枚のみ、AP は全件送信）
- [x] プロフィール更新 API にアバター・バナーの `media_file_id` 指定を追加
- [x] **投稿への動画・音声添付対応**: `media_files` の `width`/`height`/`blurhash` を
  NULL 許容化し `duration_ms`/`thumbnail_key` を新設（マイグレーション
  `20260715000000_media_files_video_audio.sql`）。`create_drive_file`
  （`crates/seiran-api/src/handlers/drive.rs`）でマジックバイト判定（`infer`
  クレート、新規 `storage/media_probe.rs` の `sniff_mime_type`）により画像/動画・
  音声を判別し、動画・音声は許可 MIME ホワイトリスト（mp4/webm/mov、
  mp3/ogg/wav/m4a/flac）チェック後、`ffprobe`/`ffmpeg`（shell out、
  `probe_video_or_audio`）でメタデータ抽出とサムネイルフレーム抽出のみ行う
  （トランスコードはしない、原本はそのままS3保存）。サムネイルは既存の
  `process_image(_, MediaKind::Post)` に通してWebP化・blurhash計算し別
  storage_keyで保存。AP配信（`ap/deliver.rs`）は既存の `Document`/`mediaType`
  がそのまま機能し、`blurhash` フィールドも追加。ATP(Bsky)配信
  （`atp/service.rs` の `commit_post`）は画像が無く動画・音声のみの場合
  `app.bsky.embed.external`（URLカード）にフォールバック
  （`app.bsky.embed.video` 本格対応は uploadBlob・ES256Kサービス間認証JWTが
  必要な別タスクとしてスコープ外）。フロントは `PostComposer.tsx` の
  accept を拡張し `<video>`/`<audio>` プレビューに対応、`NoteCard.tsx` は
  Fedi受信動画・音声対応（前回コミット）のロジックをそのまま再利用。
  アップロード上限を100MBに変更（axum `DefaultBodyLimit`・nginx
  `client_max_body_size`・Dockerfileへの`ffmpeg`追加）。

### 初回セットアップ
- [x] `GET /api/setup/status` — 初期化状態チェック
- [x] `POST /api/setup` — 管理者ユーザー作成（メール確認不要・PLC 同期登録）
- [x] フロントエンド: 起動時セットアップチェック・`Setup.tsx` ページ実装

### 管理画面（バックエンド API）
- [x] `role` チェックミドルウェア実装（`admin` / `moderator` でエンドポイントを保護）
  - `require_admin(&headers, &state.local_auth, state.users.as_ref()).await?` パターンで全管理 API を保護
- [x] `GET /api/admin/storage-providers` / `POST` / `PATCH` / `DELETE`
  - `secret_key` を AES-256-GCM で暗号化して格納・復号して返却
- [x] `GET /api/admin/users` — ユーザー一覧（凍結状態含む、上限 100 件）
  - `users` JOIN `actors` で `username` を取得（LEFT JOIN で actor 未作成ユーザーにも対応）
- [x] `POST /api/admin/users/:id/suspend` / `unsuspend`
  - `suspended_at = NOW()` / `NULL` を更新
- [x] `POST /api/admin/users/:id/role` — ロール変更（admin のみ）
  - `user` / `moderator` / `admin` のみ受付、`$1::user_role` キャスト
- [x] `GET/POST/PATCH/DELETE /api/admin/emojis` — カスタム絵文字管理
  - shortcode の英数字・アンダースコアバリデーション、重複時 409 Conflict
  - [x] 絵文字タグ（#49）: `custom_emojis.tags TEXT[]`。複数タグ・共有タグ可、ホワイトスペース以外の文字許可。作成/更新 API で設定、管理 UI で編集。ピッカー部分一致は将来のピッカー実装時にこのタグを消費する
- [x] **絵文字一括インポート（#50）: Misskey ZIP エクスポート形式対応**
  - [x] `custom_emojis.license TEXT` カラム追加（マイグレーション `20260705200000_custom_emojis_license.sql`）
  - [x] `POST /api/admin/emojis/import` — ZIP を受け取りバックグラウンドジョブ起動、`{jobId, total}` を返す
  - [x] `GET  /api/admin/emojis/import/:job_id` — ジョブ進捗ポーリング（`{processed, skipped, failed, done, errors}`）
  - [x] `zip` crate（v2）で meta.json 解析 → 画像処理 → S3 アップロード → DB 登録（既存ショートコードはスキップ）
  - [x] `DashMap<String, ImportJobStatus>` で AppState にジョブ状態を保持（再起動で揮発、許容）
  - [x] 管理画面 EmojisPanel に ZIP インポート UI を追加（進捗ポーリング表示）

### GC ジョブ
- [x] 未参照かつ 7 日以上経過した `media_files` を定期削除するジョブ実装
  - object storage DELETE → `media_files` DELETE の順
  - `seiran-api` の起動タスクとして組み込み（1時間ごと、最大 100 件/回のベストエフォート削除）

### SMTP 設定の管理画面化（`.env` からの移行）
- [x] `site_settings` テーブル新規作成（キーバリュー形式）
  - SMTP: `smtp_host`, `smtp_port`, `smtp_username`, `smtp_password`, `smtp_from`
  - メール確認設定: `require_email_verification`（値 `"true"` / `"false"`、デフォルト `"false"`）
- [x] 管理者 API: `GET/PATCH /api/admin/site-settings`
  - SMTP 設定はレスポンスでパスワードを返さない（`smtp_password_set: bool`）
- [x] バックエンドの SMTP 送信を `.env` から DB 設定に切り替え
  - `mailer` はリクエスト時に DB から取得する構成に変更（`SiteSettingsRepository.get_all()`）
  - SMTP 未設定時: `POST /api/auth/verify-email` は HTTP 503 + `{"code": "SMTP_NOT_CONFIGURED"}` を返す
- [x] 新規ユーザー登録フローを `require_email_verification` に連動させる
  - OFF（デフォルト）: メール確認なしで直接登録できるフォームを表示
  - ON: SMTP 設定済みが前提。既存のメール確認フローを使う
  - フロントエンドは `POST /api/meta` の `requireEmailVerification: bool` を見てフォームを切り替える
- [x] パスワードリセット機能も DB の SMTP 設定を使うよう更新
- [x] `.env.example` の SMTP 設定項目を `# DEPRECATED` コメントに変更

> **実装方針**: パスワードリセット機能（フェーズ 2.8）は `.env` 設定で先行実装し、本タスクで DB 設定に切り替えた。

---

## 🛡️ フェーズ5: 重複排除 (デデュプリケーション) ＆ マージエンジン (Deduplication)
マルチプロトコル間で生じる投稿の重複を、シナリオ別に水際で防ぐマージ・リンク機能を実装する。

- [x] **5.1. シナリオ1: 自住民の投稿の逆輸入（ループバック）の検知・リンク**
  - [x] ブリッジ等を経由して戻ってきた自サーバー住民の投稿を検知
  - [x] `parent_original_post_id` に自サーバーのオリジナル投稿IDをハードリンク
- [x] **5.2. シナリオ2: 他seiranユーザー間のマルチプロトコル投稿のマージ**
  - [x] **送信側**: 投稿作成時に `seiran_post_uuid` を自動生成し、AP Note の `seiranUuid` フィールドへ埋め込む
  - [x] **受信側（AP）**: 受信時に `seiranUuid` を DB 検索
    - [x] 未登録なら新規インサート（`seiran_post_uuid` を保存）
    - [x] 登録済みならインサートせず、既存の posts レコードの `ap_object_id` を UPDATE
- [x] **5.3. シナリオ3: 一般ブリッジユーザーの重複排除・リンク**
  - [x] 外部ブリッジによる重複投稿を許容してDBに受け入れる
  - [x] AP Note の `url` フィールドが bsky.app URL の場合、`at_uri` で既存ポストを検索して `parent_original_post_id` に紐付ける

---

## 🔍 フェーズ6: 検索ステート ＆ セッションマネージャー (Search State)
メモリやRedisを活用した検索セッションのライフサイクル管理と、API互換ページネーション・ブレンドアルゴリズムを実装する。

- [x] **6.1. `SessionStore` Trait とライフサイクル管理の実装**
  - [x] `SearchSession` 構造体（クエリ、カーソル、未返却バッファ、最終アクセス時刻）の定義
  - [x] **InMemorySessionStore**: `dashmap` を用いたスライディングタイムアウト（10分）の実装
  - [ ] **RedisSessionStore**: JSONシリアライズと Redis TTLを用いたセッション管理の実装（フェーズ8）
- [x] **6.2. 検索ページネーション・ブレンドアルゴリズムの実装**
  - [x] **初回リクエスト**: ローカルDB ＆ AppView（`searchPosts`）の同時フェッチ、織り合わせマージ、未返却バッファとカーソルのセッション格納
  - [x] **過去掘り (`session_id`)**: バッファ不足時のAppView追加フェッチ、ローカルDB追加フェッチ、再ブレンド
  - [x] **セッション消滅時**: ローカルDB限定検索への安全なフォールバック

---

## 🎨 フェーズ7: フロントエンド完成（3ペインUI） ＆ Misskey API互換レイヤー (Full UI & API)
フェーズ4.5のフロントエンドは動作確認用のスタブであり、このフェーズで一から書き直す。
独自Webフロントエンド（**React + Vite + TypeScript**）の3ペインUIと Misskey API互換レイヤーを完成させる。
`client.ts`・`AuthContext` 等のAPI定義層は流用し、UIコンポーネントは全面刷新する。

- [x] **7.1. Misskey API互換エンドポイントの拡充 (バックエンド)**
  - [x] `/api/notes/timeline`（ホームTL）、`/api/notes/search` 等の主要エンドポイントの実装
  - [x] ページネーション（`sinceId` / `untilId`）を統一ポストIDと検索セッションにマッピングする処理
- [x] **7.2. 拡張メタデータのAPIレスポンス埋め込み**
  - [x] `renoteId`, `quoteId`, `replyId`, `parentOriginalId`, `actorType` を NoteResponse に付与
  - [x] `parent_original_post_id`, `seiran_post_uuid` を posts テーブルに追加（重複排除マージキー）
- [x] **7.3. Webフロントエンド（React + Vite）の3ペインUIの実装**
  - [x] デフォルトの3ペイン構造（左240px メニュー / 中央600px / 右500px）、ダイアログ駆動の画面遷移
    - `AppShell` + `LeftNav`、投稿は `Modal` + `PostComposer` のダイアログ起動
    - 右ペインの動的コンテキスト切替（Doc5 §2）: ホーム=通知（デフォルト）/トレンド＆検索、プロフィール=投稿リスト全面、ポスト詳細=投稿主前後/リアクション
    - `RightPaneContext` でサブタブインデックスをセッション内保持（Doc5 §2.4）
  - [x] 拡張メタデータの解析ロジック（アクターがブリッジか、魂の結合済みかどうかの判別）
    - `ProfileResponse` に `bridge_real_handle` / `bridge_protocol` / `is_paired` / `at_did` / `bio` を追加
    - `NoteCard` が `renoteId` / `quoteId` / `replyId` / `parentOriginalId` を解釈してバッジ・導線を描画
    - [x] リポストのカード表示（#45）: `NoteResponse.renote` に元ポストを埋め込み、「（表示名）が（日時）にリポスト」ヘッダ＋元ポスト本体を描画。日時はリポスト自身の詳細へリンク
    - [x] リポストボタン: `NoteCard` に 🔁 ボタンを追加。`api.notes.create({renote_id})` でリポスト作成、StreamHub 経由でリアルタイムTL反映
    - [x] AP 受信リポスト（Announce）: `handle_announce()` で元ポストを DB から検索し `repost_of_post_id` 付きで保存。`Undo(Announce)` で論理削除
    - [x] リポスト解除の完全実装: `DELETE /api/notes/:id/repost` → AP Undo(Announce) 配送 + ATP `app.bsky.feed.repost` delete commit。`posts.atp_repost_rkey` カラムで rkey を保持し解除時に特定
  - [x] UIの実装:
    - [x] フォロー警告モーダル（ブリッジアカウントフォロー時の警告・Doc5 §3.2）
    - [x] 「本尊ワープ」ボタン（ブリッジアカウントから本尊へシームレスにジャンプ・Doc5 §3.1）
  - 注: トレンド集計はバックエンド未実装のためプレースホルダ表示（実装後に有効化）。通知は下記「通知の永続化」項目で実装済み
  - [x] **絵文字リアクション機能（ローカル実装）**: `POST/DELETE /api/notes/:id/reactions[...]` でローカルユーザーが絵文字リアクションを追加/取消（`ReactionRepository::delete_local` 追加）。`NoteResponse.reactions[].reactedByMe` で自分の付けたリアクションを判定。フロントは `ReactionChips`（表示チップをクリックで同じ絵文字をトグル/切替）と `ReactionPicker`（固定クイック絵文字 + 自由入力）を実装。AP/ATP への outbound 配信は以下の項目で追加実装済み
  - [x] **1投稿1ユーザー1リアクションへ統一**: `reactions.UNIQUE(post_id, actor_id)`（マイグレーション `20260710010000_reactions_one_per_actor.sql`）に変更し、`ReactionRepository::insert` を `ON CONFLICT DO UPDATE` による切り替えに変更（Misskey 準拠、投稿元がローカル/リモートでも挙動を統一）
  - [x] **リアクションのリアルタイム配信**: `seiran_common::streaming::broadcast_reaction_update`（新設、`ReactionRepository::aggregate_for_post` で集計）を local API（`create_reaction`/`delete_reaction`）と AP 受信（`handle_reaction`/`Undo(Like/EmojiReact)`）の両経路から呼び、`{"type":"noteUpdated","body":{postId,reactions,reactorActorId,reactorEmoji}}` を著者+accepted なローカルフォロワーへ WebSocket 配信。フロントは `StreamingContext` に `registerReaction(noteId, cb)` を追加し `NoteCard` が購読、自分の操作かどうかは `/api/auth/me` 等のレスポンスに追加した `actor_id` で判定。既知の制約: フォロー関係の無い閲覧者（ノート詳細を直接開いた場合等）には届かない（「今見ている人」を追跡する仕組みが無いため。新規ポスト配信と同じ既存の制約）
  - [x] **リアクションの ATP（Bluesky）相互配送**: OUTBOUND は `AtpCommitService::commit_like`/`delete_atp_like`（新設）で `app.bsky.feed.like` をコミット/削除（絵文字は非標準 `emoji` 拡張フィールド、実際に生成した署名付きレコードで UTF-8 埋め込みを確認済み）。`reactions.at_uri` に自己保存し切替時は旧 Like を削除してから作り直す。INBOUND は `seiran-atp-repo` に `app.bsky.feed.like` の create/delete 検知を追加。ATP には絵文字概念が無いため常に Like（受信側は `content="❤️"` 既定、絵文字ピッカーと同じVS16付きハート）として扱う。
  - [x] **リアクションの AP（ActivityPub）相互配送（送信側）**: OUTBOUND を `crates/seiran-common/src/ap/deliver.rs` の `deliver_ap_reaction`/`deliver_ap_undo_reaction`（新設）で実装。配送先は「対象ポストの著者（`actor_type='fedi'` の場合のみ）」と「リアクションした本人の Fedi フォロワー全員」の inbox の和集合（他のアクティビティ配送と同じ sharedInbox 未対応のバラ配送パターン）。リアクション内容が `❤️` の場合は `Like`、それ以外は `content`/`_misskey_reaction` 付きの `EmojiReact` として送信。切替時は `reactions.ap_activity_id`（新設、ローカル発行の activity URI を保存）を使って旧リアクションを先に `Undo` してから送り直す。`ReactionRepository::find_at_uri` は `find_current`（content/ap_activity_id/at_uri をまとめて返す）に統合。INBOUND（`handle_reaction`/`Undo(Like/EmojiReact)`）は既存実装のため、これで送受信が双方向になった
  - [x] **リアクション内容を Unicode 絵文字のみに制限するバリデーション**: `validate_reaction_content`（`crates/seiran-api/src/handlers/notes.rs`）を、書記素数チェックのみの緩い検証から `emojis` crate（Unicode公式 `emoji-test.txt` 準拠、新規依存追加）による完全一致チェックへ変更。単体絵文字だけでなく肌色/性別修飾・ZWJ結合・国旗・キーキャップ等の RGI シーケンスも許可しつつ、プレーンテキストや `:shortcode:` は拒否する（カスタム絵文字ピッカー未実装のため、ローカル送信経路では意図的に絵文字のみへ制限）。フロントも `frontend/src/lib/reaction.ts`（`isValidReactionEmoji`、npm パッケージ `emoji-regex` 新規追加）で `ReactionPicker` の自由入力欄を同方針で事前チェックし、不正な入力は送信ボタンを無効化してエラーメッセージを表示する。AP `EmojiReact` の INBOUND（他インスタンスのカスタム絵文字ショートコードを含みうる）は対象外で従来通り無検証のまま保存する
  - [x] **Fedi受信リアクションの内容解決修正・カスタム絵文字対応**: `handle_reaction`（`crates/seiran-federation-inbox/src/handlers/inbox.rs`）で、Misskey が絵文字リアクション（Unicode/カスタム絵文字問わず）でも AP `type` を `"Like"` 固定で送り `EmojiReact` 型を使わないために、これまで wire type ベースの `is_like` 判定で全件 `❤️` 固定になっていた不具合を修正。`content`（無ければ `_misskey_reaction`）の値で内容・`reaction_type` を決めるよう変更し、`content` が `:shortcode:` 形式ならカスタム絵文字とみなし activity の `tag` 配列（`type:"Emoji"`）から画像 URL を解決する新規ヘルパー `extract_emoji_tag_url` を追加。マイグレーション `20260713000000_reactions_emoji_url.sql` で `reactions.emoji_url TEXT` を新設し、`ReactionRepository::insert`/`aggregate_for_post` のシグネチャに反映。API 公開層（`ReactionSummary`/`fetch_reactions_map`）・WS配信（`broadcast_reaction_update`）にも `emojiUrl` を通す。フロントは `ReactionChips` に `emojiUrl` があれば `img` 表示する分岐を追加し、ローカルユーザーが自分でまだ付けていないカスタム絵文字チップ（送信すると Unicode 限定バリデーションで必ず拒否される）はクリック不可にする
  - [x] **通知欄・投稿本文・表示名中のカスタム絵文字を画像表示**: (1) 通知欄: `handle_reaction` で計算済みだった `emoji_url` を WS `"reaction"` イベントのペイロードに配線し忘れていた不具合を修正し `emojiUrl` を追加、`NotificationsPanel.tsx` が `img` 表示するよう対応。(2) 投稿本文・表示名: `seiran_common::ap::build_emoji_map`（新設、`ApActor`/Note いずれの `tag` 配列からも `{shortcode: 画像URL}` を構築する共通関数。9.6節の `extract_emoji_tag_url` もこれを使うよう統一）を追加。マイグレーション `20260713010000_note_actor_emoji_map.sql` で `posts.emoji_map`/`actors.emoji_map`（JSONB）を新設し、`handle_create_note`/`upsert_remote_fedi_actor`（`follows.rs` のフォロー時解決からも）で AP 受信時に保存。`PostRepository::insert_remote_with_dedup`/`ActorRepository::upsert_remote_fedi` のシグネチャに反映し、`TimelinePost` に `post_emoji_map`/`actor_emoji_map` を追加して該当する全SQLクエリ（`home_timeline` 等）のSELECT句を更新。`NoteResponse.emojis`（投稿+投稿者のマップを統合）としてAPI公開し、フロントの新規コンポーネント `EmojiText.tsx` が本文・表示名中の `:shortcode:` をこのマップで `img` に置換する（`NoteCard` の本文・投稿者名・リポスト元表示名に適用）。既知の制約: `ProfilePage` 単体の表示名・`ReplyIndicator` の返信先表示名は対象外（今後の課題）
  - [x] **投稿・Like取り込みを Jetstream へ移行**: `seiran-atp-repo` を Relay 生 Firehose（CBOR/CAR直結、全collection購読）から Bluesky公式の **Jetstream**（`wss://jetstream1.us-east.bsky.network/subscribe`、`wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.feed.like` でサーバー側collectionフィルタ済みJSON配信）へ移行。CBOR/CARの自前デコード（`find_car_block` 等）が丸ごと不要になったため削除。投稿取り込みはJetstreamが同梱する `record.text`/`record.createdAt` を直接使うようになり、旧実装にあった AppView 再取得＋インデックス遅延リトライ（2s/5s/10s）が不要に。既知の制約: zstd圧縮は今回未導入（今後の課題）。Likeは対象が任意の投稿のためDIDでは絞り込めず、グローバルなLikeイベント全件（実測概ね600〜700件/秒）を判定する設計は変わらない。
  - [x] **fix: サーバー停止中に発生したJetstreamイベントを取りこぼす不具合を修正**: 上記移行時に`cursor`パラメータを付与しておらず、プロセス起動・再接続のたびに「今この瞬間」からのライブ配信として接続していたため、デプロイ・クラッシュ等でのサーバー停止中に発生した投稿・Likeイベントが再起動後に取得できず失われていた。専用テーブルは設けず、既存の汎用KVテーブル`site_settings`（Doc1 §1.11、キー`jetstream_cursor`）に、受信メッセージ全種別共通の`time_us`（マイクロ秒Unixタイムスタンプ）を5秒間隔で保存し、接続開始時に読み出して`&cursor=<time_us>`をURLに付与する方式で対処（Doc3 §14.2）。
  - [x] **fix: フォロイー数に関わらずBluesky全体のpost/likeイベント全件に対してDBクエリが発行されるパフォーマンス問題を軽減**: DID絞り込みを行わず`wantedCollections`のみでJetstreamに接続していたため、無関係な投稿・いいねイベント全件に対してもDBフィルタクエリ（`follows`/`list_members`のEXISTS判定）が発行されていた。`load_wanted_dids`（`crates/seiran-atp-repo/src/firehose.rs`）が、ローカルユーザーのフォロー先またはリストメンバーであるBskyアクターのDID一覧（退会済みユーザーのフォロー・所有リストは除外）を取得し、`wantedDids`パラメータ（上限10,000 DID）としてJetstream接続URLに付与するよう変更。DID集合はフォロー・リストメンバーの増減で動的に変わるため、新設`seiran_common::jetstream_control`が`site_settings`（キー`jetstream_wanted_dids_touch`）の`updated_at`を「変更バージョン」として使い、DBポーリング（30秒間隔）でプロセス間に変更を通知する仕組みを追加（split-role構成で`firehose`ロールが`api`ロールと別プロセスで動くため、プロセス内通知は使えない）。トリガー箇所: ATPフォロー作成・解除（`follows.rs`）、リストメンバー追加・削除・リスト削除（`lists.rs`）、ローカルユーザー退会（`account.rs`）。既存のDB側フィルタは、絞り込みリスト更新の反映ラグに対する保険として維持（Doc3 §14.2）。`load_wanted_dids`は当初`actors`から出発しEXISTSで判定する書き方で実装し実測1秒近くかかっていたが、`follows`/`list_members`を起点にJOINでDIDを引く書き方に修正し0.5ms台まで短縮（実機確認・2026-07-16）。
  - [x] **fix: `firehose`ロール（Jetstream接続）を複数インスタンス起動した場合の排他制御を追加**: `docker-compose.mono.yml`の`--scale seiran-server=N`（無停止バージョンアップ中の一時的な複数起動）や`firehose`ロールの複数インスタンス起動で、Jetstream WebSocket接続がインスタンス数だけ重複して張られる問題（2026-07-16 マイケル指摘）に対処。新設`seiran_common::jetstream_leader`（`JetstreamLeaderElector`）がRedisのTTL付きリース（`SET NX EX 10`）でリーダーを1つに絞る。プロセスIDではなくUUID（Dockerコンテナ間でPID 1が衝突するため）でリーダーを識別し、`firehose.rs`の制御ループが5秒間隔でリースの取得・延長を試み、成否に応じてJetstream接続タスクを`tokio::spawn`/`abort`で起動・停止する。TTL延長は「現在の値が自分のUUIDと一致する場合のみ延長」をLuaスクリプト（`EVAL`）でアトミックに行い、GET→SET非アトミックによるsplit-brainの理論的な穴を解消。Redis呼び出し（接続確立・リース確認）には3秒のタイムアウトを設定（`redis`クレートの`ConnectionManager::new`がRedis無応答時に内部リトライで長時間ブロックし、ポーリングループ自体が停止する不具合を実機検証で発見し対処）。Redis未設定・通信失敗時（両者は「Redisと通信できない」の特殊ケースとして同一視）は、`all`ロールはJetstream接続を維持（フェイルオープン、monolithの複数起動時の非効率は許容する方針）、`firehose`ロール（split-role構成）は切断する（フェイルクローズ、Redisが死ねばジョブキュー等の他機能も共倒れになるため）。実機確認: 2プロセス起動→片方停止→もう片方への昇格、Redis障害中のフェイルオープン継続（ポーリングがハングせず5秒間隔でエラーを出し続ける）、REDIS_URL未設定時の従来動作維持を確認済み（2026-07-16）。Redis復旧後の再昇格は検証環境のネットワーク制約で直接確認できなかったが、コードパスはRedis未使用時の初回接続と同一のため論理的には問題ない。Redisダウンによる多台体制の崩壊は人手でのインフラ復旧が前提であり、自動復帰までは要求していない（マイケル判断）。
  - [x] **fix: Likeの通知insertが複線受信で重複する不具合を修正**: マイグレーション`20260716060000_notifications_source_uri.sql`で`notifications.source_uri`（発生源イベントの一意識別子）カラムと部分ユニークインデックス（`WHERE source_uri IS NOT NULL`）を新設し、`NotificationRepository::insert`に`source_uri: Option<&str>`引数を追加、SQLを`ON CONFLICT (source_uri) WHERE source_uri IS NOT NULL DO NOTHING`に変更。ATP Like通知（`firehose.rs`の`handle_inbound_like_create`）は`at_uri`を、AP Reaction通知（`inbound_activity_process.rs`の`handle_reaction`）は`ap_activity_id`を渡す。follow系・ローカルリアクションの通知呼び出し3箇所は`None`のまま（NULL同士は一意制約上区別されるため無関係）。実機確認: 同一`source_uri`で2回INSERTし1行のみ残ることを確認済み（2026-07-16、Doc1 §1.12・Doc3 §14.2）。
  - [x] **fix: Bskyからの取り込みでリプライが通常投稿として保存される不具合を修正**: `record.reply.parent.uri` を無視していたため、seiranユーザーの投稿へのBskyリプライが `reply_to_post_id = NULL` の単純投稿として保存されていた。`record.reply.parent.uri` を見て、親投稿が `posts.at_uri` として既知なら `reply_to_post_id` を設定するよう修正（未知の親なら従来通り通常投稿として保存）。既存の誤取り込みデータ1件（`https://bsky.app/profile/yuba.bsky.social/post/3mqj72ktvnk2o`）はBluesky公開APIで実レコードを再取得し手動バックフィル済み。
  - [x] **fix: Bsky投稿の画像・動画添付が一切取り込まれない不具合を修正**: `firehose.rs`（Jetstream受信）が `record.embed` を一切参照しておらず、Bsky発の投稿は常に `attachments: []` になっていた。新規 `parse_bsky_embed_attachments`（`crates/seiran-atp-repo/src/firehose.rs`）で `record.embed` を解析し、`app.bsky.embed.images` は Bluesky CDN画像URL（`https://cdn.bsky.app/img/feed_fullsize/plain/{did}/{cid}`）、`app.bsky.embed.video` はBluesky公式動画パイプラインが生成するHLSプレイリストURL（`https://video.bsky.app/watch/{did}/{cid}/playlist.m3u8`）とサムネイルJPEG URLを、DID+blob CIDのみから決定的に組み立てる（Bluesky AppViewへの追加問い合わせ不要）。`app.bsky.embed.recordWithMedia`（引用+メディア）は`media`フィールドを再帰的に解決。マイグレーション `20260715100000_post_attachments_remote_thumbnail.sql` で `post_attachments.remote_thumbnail_url` を新設し、`attach_remote_media_url`（`PostRepository`）のシグネチャに反映。WebSocketリアルタイム配信（`noteUpdated`ではなく新規ポスト配信の`attachments`フィールド）にも実データを反映。フロントは新規 `HlsVideo.tsx`（`hls.js` 新規依存）で、SafariはネイティブHLS再生、それ以外（Chrome/Firefox等）は`hls.js`経由のMediaSource Extensions再生に分岐する `NoteCard.tsx` の動画描画を拡張。
  - [x] **fix: Fediリモート投稿の動画・音声添付がnotecardで再生できない不具合を修正**: リモート受信の添付は `post_attachments.remote_url` のみを保存しURLの種別を一切保持していなかったため、API レスポンス生成側（`fetch_attachments_map`、`crates/seiran-api/src/handlers/notes.rs`）が `COALESCE(mf.mime_type, 'image/jpeg')` で常に `image/jpeg` を返し、フロントも `NoteCard.tsx` が無条件に `<img>` でレンダリングしていた（動画・音声は本文しか表示されない）。マイグレーション `20260714000000_post_attachments_remote_mime_type.sql` で `post_attachments.remote_mime_type TEXT` を追加し、`handle_create_note`（`crates/seiran-federation-inbox/src/handlers/inbox.rs`）が AP attachment の `mediaType` を保存（欠落時は新設ヘルパー `guess_attachment_mime_type` がURL拡張子から推測、判別不能なら `NULL`）するよう変更。`fetch_attachments_map` は `COALESCE(mf.mime_type, pa.remote_mime_type, 'image/jpeg')` に変更（`image/jpeg` は媒体種別が全く分からない過去データ向けの最終フォールバックとして維持）。フロントは `NoteCard.tsx` が `mimeType` の `video/`/`audio/` prefix で `<video controls>`/`<audio controls>` に分岐するよう変更。
  - [x] **通知の永続化（Misskey API互換 `POST /api/i/notifications`、Doc3 §5.8・Doc1 §1.12）**: 従来クイック通知はWS配信のみで永続化しておらず、フロントのインメモリ配列（最大100件、リロードで消滅）に頼っていた。マイグレーション `20260716010000_notifications.sql` で `notifications` テーブルを新設し、`NotificationRepository`（`seiran-common::repository::notification`）を `AppState`/`InboxContext` にDIした上で、リアクション作成（ローカル/AP/ATP inbound）・AP `Follow`/`Accept(Follow)` 受信の各経路から書き込むよう変更。読み取りは本家 Misskey と同じワイヤープロトコルの `POST /api/i/notifications`（`handlers::misskey::endpoints::i_notifications`、`until_id`/`since_id`カーソル）で公開。フロント `NotificationsPanel.tsx` は初回20件をREST取得後、下端到達で`IntersectionObserver`により`untilId`で過去分を追加取得する無限スクロールに全面書き換え。`StreamingContext`のWSライブ通知は「新着シグナル」のみに用途変更し、実データは常に`sinceId`付きREST再取得で一覧と同じID体系に統一（従来の`Notif`型・インメモリ配列は廃止）。初回実装時はリアクションのカスタム絵文字画像URLを解決できず通知一覧が常にテキスト表示になる回帰があったが（#405で実装したWS版`emojiUrl`表示が失われていた）、本家Misskeyの実装（`reactionEmojis`は通知オブジェクトではなく同梱`note`側が持つ）に合わせて`MisskeyNote.reactionEmojis`を追加し即日修正（Doc3 §5.8）。さらにその実装は投稿の「現在の」リアクション集計から都度解決する方式だったため、同じアクターがリアクションを切り替えると過去の通知が再び解決不能になる不具合があり、`notifications.reaction_emoji_url`（マイグレーション`20260716020000_notifications_reaction_emoji_url.sql`）に通知発生時点のURLを非正規化保存する方式へ即日再修正（Doc1 §1.12・Doc3 §5.8）。既知の制約: (1) この再修正前に作成された通知データは遡って修正されない。(2) ローカル同士のフォローはAPを経由しないため通知が発生しない（リモートからのフォローのみ対応）。
- [x] **7.3.x. 退会機能 Phase A（#29）**
  - [x] `actors.withdrawn_at` カラム追加（マイグレーション）
  - [x] `POST /api/account/withdraw`（`confirm_handle` 必須、AP Delete(Actor)配送 + ATP #account broadcast + 全投稿論理削除 + `withdrawn_at` セット）
  - [x] ATP subscribeRepos `#account` フレーム生成（`build_account_frame`）+ `broadcast_account_event`
  - [x] AP `deliver_delete_actor` — Fedi フォロワー全員に Delete(Actor) 送信
  - [x] フロントエンド: プロフィール編集画面の下部に「退会」セクション（ハンドル確認入力必須）
  - [x] **fix: 退会時に自分がフォローしていた相手（フォロイー）側への関係解消通知が無かった不具合を修正**: 従来`deliver_delete_actor`は`fetch_fedi_follower_inboxes`で自分のFediフォロワーにのみDelete(Actor)を配送しており、フォロイー側のリモートサーバーには何も届かず、ATP側も`broadcast_account_event`のみで`app.bsky.graph.follow`の個別解除は送っていなかった（2026-07-16 マイケル指摘）。フォロー数に比例する処理時間を考慮し、新設`Job::AccountWithdrawUnfollowAll`（`seiran_common::jobs::account_withdraw_unfollow_all`）をWorkerのジョブとして実装（`ApDelivery`/`ProxyFollowSync`と同じリトライ設定: 最大10回、5秒〜1時間の指数バックオフ）。`account::withdraw`は`AppState::enqueue_account_withdraw_unfollow_all`でジョブを積むのみ。ジョブハンドラは`FollowRepository::find_accepted_target_ids`で取得した全フォロー先について、ATPフォロー解除コミット・AP Undo Follow配送・`follows`削除を行う（`follows`行は処理の最後に削除するため、リトライ時は処理済みターゲットが自然にスキップされる冪等設計）。当初`tokio::spawn`で非同期化する実装だったが、プロセスクラッシュ時にタスクごと失われリトライもされない点を指摘され（2026-07-16 マイケル）、既存のジョブキューパターンに載せる設計に変更した。実機確認: ATP followレコード付きのローカルフォローを持つテストユーザーを退会させ、`Worker`ログでのジョブ実行とATP `follow delete commit`実行（rkey一致）・`follows`行の削除を確認済み（2026-07-16、Doc3 §14.2）。
- [x] **7.3.y. ポストのピン留め機能（#61）**
  - [x] `pinned_posts` テーブル新設（マイグレーション `20260716030000_create_pinned_posts.sql`、Doc1 §1.13）。`PinnedPostsRepository::pin` が5件超過時に最古から自動追い出し
  - [x] `POST`/`DELETE /api/notes/:id/pin`（自分の投稿のみ）、`ProfileResponse.pinned_posts`/`NoteResponse.pinned_by_me` を追加
  - [x] Fedi 送信: Actor `featured` フィールド + `GET /users/:username/collections/featured`（`OrderedCollection` を都度動的生成）
  - [x] Fedi 受信: リモートアクターの featured collection を取得・取り込み、プロフィール表示のたびに `pinned_posts` を同期（`sync_remote_fedi_pinned`）。DB 未登録の未知アクター（初回アクセス）でも `fetch_remote_profile` がその場で actor を upsert してから同期するよう対応（マイケル実機確認・2026-07-15）
  - [x] Bsky 送信: `app.bsky.actor.profile` の `pinnedPost`（strongRef）に最新1件を反映（`AtpCommitService::commit_profile` 拡張）
  - [x] Bsky 受信: リモートアクターの `pinnedPost` を `fetch_single_bsky_post` で取得・取り込み、`pinned_posts` を同期（`sync_remote_bsky_pinned`）
  - [x] フロントエンド: プロフィール画面中央ペインにピン留めセクション、右ペインは従来通り最新ポスト（狭幅では中央ペインに連続表示）、`NoteCard` にピン留めトグルボタン（Doc3 §13）
  - [x] fix: AP Note の `content`（HTML）が `strip_html` されずタグ付きのまま保存される既存不具合を `upsert_ap_note`/`ActorHistorySync::save_ap_notes` の双方で修正（本機能の実機確認中に発見）
- [x] **7.3.z. プロフィールのキーバリュー項目（#62）**
  - [x] `actors.profile_fields` カラム新設（マイグレーション `20260716040000_actors_profile_fields.sql`、Doc1 §1.2）。最大 `MAX_PROFILE_FIELDS`（4）件
  - [x] `PATCH /api/users/profile` に `profile_fields` を追加（バリデーション: 件数上限・空行除外・ラベル/値の文字数上限）、`ProfileResponse.profile_fields` を追加
  - [x] Fedi 送信: Actor `attachment`（`PropertyValue`、URL は `<a rel="me">` でリンク化）
  - [x] Bsky 送信: `fetch_atp_profile_material`/`append_profile_fields_to_bio` で bio 末尾にリスト追記（構造化フィールドが無いための代替表現）
  - [x] Fedi 受信: `ApActor.attachment` から `profile_fields` を取り込み、`upsert_remote_fedi` の全呼び出し経路（Follow受信・フォロー時解決・未認知アクター初回アクセス）で反映
  - [x] フロントエンド: `/settings/profile` に固定4行の編集フォーム、プロフィール画面に表示ブロック（DID/URIと同じスタイルを再利用）
  - [x] fix: `ActorRepository::upsert_remote_fedi` が `bio` を一切設定しない既存不具合を修正（`bio: Option<&str>` 引数を追加、`avatar_url` と同じ COALESCE パターン）。#61 で未認知アクターも upsert してから返す設計に変更した際にこの欠陥を踏み抜き、リモート Fedi アクターの自己紹介文が表示されなくなる退行を生んでいた（マイケル報告・2026-07-15）
- [x] **7.3.zz. リスト機能（#63）**
  - [x] `lists`/`list_members` テーブル新設（マイグレーション `20260716050000_create_lists.sql`、Doc1 §1.14）。上限 `MAX_LISTS_PER_OWNER`(30)/`MAX_MEMBERS_PER_LIST`(500)
  - [x] `ListRepository`/`PgListRepository`（`crates/seiran-common/src/repository/list.rs`）、`GET/POST/PATCH/DELETE /api/lists(/:id)`・メンバー追加削除・タイムライン取得（`handlers::lists`）
  - [x] list-relay プロキシアクター（`seiran_common::system_actor`、起動時に冪等生成）＋参照カウント方式のプロキシフォロー（`Job::ProxyFollowSync`、`jobs::proxy_follow_sync`）
  - [x] ユーザー名のDNSラベル準拠バリデーション＋予約ユーザー名拒否（`seiran_common::username`、`register()`に組み込み、Doc1 §1.2）
  - [x] Fedi 送信: `GET /users/:username/lists`・`GET /users/:username/lists/:list_id`（`OrderedCollection`、`actor_type <> 'bsky'`でフィルタ）、Actor `lists` フィールド追加（`handlers::lists`, `seiran-federation-inbox`）
  - [x] Bsky 送信: `app.bsky.graph.list`/`app.bsky.graph.listitem` を自前PDSリポジトリへ実コミット（`encode_bsky_graph_list`/`encode_bsky_graph_listitem`、`AtpCommitService::commit_graph_list`/`commit_graph_listitem`/削除系）。公開リストかつ`actor_type <> 'fedi'`なメンバーのみ対象
  - [x] Bsky 受信フィルタ: Jetstream の保存可否判定・WebSocket配信対象抽出に「いずれかのリストに含まれるDID」を追加（`seiran-atp-repo/src/firehose.rs`）
  - [x] フロントエンド: `ListsSettingsPage.tsx`（`/settings/lists`）、`HomePage.tsx`のタブ拡張（横スクロール対応）、`ProfilePage.tsx`の公開リストセクション、`ListDetailPage.tsx`（`/lists/:id`）
  - [x] fix（マイケルフィードバック）: `LeftNav.tsx`に「リスト」への直接リンクを追加（`/settings/lists`の手打ちが不要に）
  - [x] fix（マイケルフィードバック）: `ListsSettingsPage.tsx`が中央ペイン内で2カラム分割していたのを3ペイン構成に沿う形に修正（中央=一覧、右ペイン=選択中リストの編集・メンバー管理、狭幅では`ProfilePage.tsx`と同じ`matchMedia`判定でフォールバック表示）
  - [x] メンバー追加のサジェスト機能: `GET /api/actors/search`（`handlers::actor_search`、ユーザー名/表示名の部分一致、list-relay等のシステムアクターは除外）＋フロントの300msデバウンス付きドロップダウン。`@user@domain`形式の入力にも対応（先頭`@`除去＋`username||'@'||domain`結合列検索、マイケル報告により追加修正）
  - [x] 実機確認: `@yuba@misskey.io`等マイケル本人のアカウントでプロキシフォロー（Follow→Accept→参照カウント維持→Undo Follow）、Bluesky実アカウントで`app.bsky.graph.list`/`listitem`のCARコミット・削除を確認済み
  - 既知の制約: リモートFedi/Bskyユーザー自身の公開リストをオンデマンド取得・表示する機能は未実装（`public_lists`はローカルユーザーのみ）。公開済みリストの名前変更はATP側レコード内容に追従しない（Doc2 §2.13, Doc3 §14）
- [ ] **7.4. サードパーティ製クライアント（ZonePane、Miria、Aria 等）向けの互換対応**
  - [ ] APIレスポンス送信時、アクターの `bio`（自己紹介）の末尾に本尊のURLを自動挿入するフォールバックロジックの実装
  - 注意: MiAuth ログイン互換対応（`/api/meta` 追加・check URL 修正）はフェーズ 2.4 で先行実施済み
  - **2026-07-13 監査**: フロントエンド向け API 全体を Misskey API と突き合わせた結果、
    ログイン導線（MiAuth・`/api/meta`）以外はほぼ非互換と判明（詳細は Doc3 §5.5）。
    - [x] `GET /api/emojis`（未認証・Misskey互換形状）の追加。従来 `/api/admin/emojis` のみで一般公開されていなかった
    - [x] `/api/meta` に `emojis` / `maxNoteTextLength` / `disableRegistration` を追加
    - [x] エラーレスポンスに Misskey 風 `error: {code, message}` を追加（既存 `code` は後方互換維持）
    - [x] Doc3 §5.3 の古い注記（check URL 修正要）を実装済みの記述に更新
  - **マイケルの方針決定（2026-07-13）**: フル互換を目指す。最終的には Misskey スキーマへ統一するが、
    移行中は既存カスタム API・自社フロントエンドを無理に同時に壊さず、有利なら並存させてよい。
    Misskey 本家の設計自体が非効率・不自然な箇所は互換用＋実用の2エンドポイント併存も許容する。
  - **Phase 1: 認証方式の二重化**
    - [x] `middleware::misskey_auth_bridge`（新規）: `Authorization` ヘッダーが無い場合に限り、JSON ボディの `i` またはクエリの `i` を検出して `Authorization: Bearer` ヘッダーへ合成。既存ハンドラは無改修
  - **Phase 2: Misskey 準拠エンドポイントの追加（既存カスタムAPIとは並存、削除していない）**
    - [x] `handlers::misskey` モジュール新設。`MisskeyNote`/`MisskeyUserLite`/`MisskeyUserDetailed`（Doc3 §5.6）
    - [x] `POST /api/i`, `POST /api/users/show`, `POST /api/notes/show`
    - [x] `POST /api/notes/local-timeline`, `POST /api/notes/timeline`（ボディで `limit`/`sinceId`/`untilId`）
    - [x] `POST /api/notes/reactions/create`, `POST /api/notes/reactions/delete`（既存ハンドラへ委譲、成功時204）
    - [x] `POST /api/notes/unrenote`（既存の repost 削除ハンドラへ委譲、成功時204）
    - [x] `POST /api/following/create`, `POST /api/following/delete`（`userId`→ローカルusername/DID/AP URIへ変換して既存ハンドラへ委譲、成功時204）
    - 既知の簡略化: `visibility` 常に `public`、`cw` 常に `null`、書き込み系のエラー形状は Misskey 本家のエラーID体系を再現していない（詳細 Doc3 §5.6）
  - **Phase 3（未着手）**: ストリーミングプロトコルのチャンネル購読対応（現状は認証ユーザー宛てブロードキャストのみ）
  - **Phase 4（未着手）**: フロントエンド（`frontend/`）の Misskey スキーマへの追従改修、検証が済んだ旧カスタムエンドポイントの整理

---

## 🧪 フェーズ8: 統合テスト ＆ 最適化 (Test & QA)
システム全体の信頼性、高負荷時の挙動、および物理分割の検証を行う。

- [ ] **8.1. 単体・結合テストの作成**
  - [x] 結合テスト基盤の新設（`crates/seiran-api/tests/`。実 DB + 本物の `seiran_api::router` を使い、
    `#[ignore]` で通常の `cargo test` からは除外し `cargo test -p seiran-api --test <name> -- --ignored`
    で明示実行する運用。`support/mod.rs` に AppState 構築・ログイン・JSON アサーションのヘルパーを整備。
    最初のテスト対象として notes（作成/取得の往復、未認証401・404がJSON形式で返ることの回帰確認）を実装。
    2026-07リファクタリングで実装）
  - [ ] 重複排除（シナリオ2のマージ処理）のユニットテスト
  - [ ] 未来補正タイムスタンプの採番テスト
  - [ ] 検索ブレンドアルゴリズムの挙動テスト
- [ ] **8.2. 連合（Federation）統合テスト**
  - [ ] モックAPサーバー、モックATPサーバーを用いた連携テスト
  - [ ] リモートseiran同士のハンドシェイクおよび特権同期のテスト
- [ ] **8.3. 高負荷・スケールアウト検証**
  - [ ] `AUTH_PROVIDER=auth0` ＋ `RedisJobQueue` ＋ `RedisSessionStore` を用いた環境での動作確認
  - [ ] プロダクションビルド・デプロイ手順の検証
