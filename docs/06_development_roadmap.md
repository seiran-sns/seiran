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
- [ ] **2.8. パスワードリセット機能**
  - [ ] マイグレーション: `password_resets` テーブル（`id, user_id, token UUID, expires_at 1h, created_at`）
  - [ ] `POST /api/auth/request-password-reset` — メールアドレスを受け取りリセットリンクを送信（ユーザー不在でも同一レスポンス）
  - [ ] `GET /api/auth/verify-reset-token?token=...` — トークン検証（副作用なし）
  - [ ] `POST /api/auth/reset-password` — `{ reset_token, new_password }` でパスワード更新・トークン消費
  - [ ] フロントエンド: `/forgot-password` ページ（メール入力）、`/reset-password?token=...` ページ（新パスワード設定）
  - [ ] エラーコード追加: `RESET_TOKEN_INVALID`, `PASSWORD_TOO_SHORT`
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
  - [x] `InMemoryJobQueue` の実装 (開発初期用、オンメモリ)
  - [ ] `RedisJobQueue` の実装 (本番スケール用、フェーズ8にてRedis連携時に実装予定)
- [x] **3.3. 5大非同期ジョブハンドラの実装**
  - [x] **① 過去ログ同期キュー (`actor_history_sync`)**
    - [x] ドメイン単位 of 同時実行制限（Concurrency Limit）の適用
    - [x] 1〜3秒のジッター挿入、指数バックオフで最大3回リトライ
  - [x] **② 投稿配送キュー (`outbound_post_delivery`)**
    - [x] 高優先度処理、相手サーバーダウン時の長期指数バックオフ（最大10回リトライ）
  - [x] **③ 配送受け入れ（インバウンド）キュー (`inbound_activity_process`)**
    - [x] ドメイン単位のレート制限、依存リソース未解決時の再スケジュール
  - [x] **④ アクター検証・メタデータ取得キュー (`actor_metadata_resolve`)**
    - [x] `/verify-actor` ハンドシェイク検証、Webfinger解決、アバター画像等のキャッシュ
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
    - [x] `com.atproto.server.describeServer` エンドポイント実装（Relay の PDS 検証用）
    - [x] `/.well-known/did.json` エンドポイント実装（サーバー DID `did:web:{domain}` 提供）
    - [x] 初回コミット時に `com.atproto.sync.requestCrawl` で Relay (`bsky.network`) へ通知（200 OK 確認済み）
    - [x] ユーザー登録時に `app.bsky.actor.profile/self` をコミット（AppView へのアクター認識に必須）
    - [x] `atp_records` テーブル追加（profile 等 non-post レコードを管理し MST 再構築に使用）
    - [x] `com.atproto.identity.resolveHandle` エンドポイント実装（Relay のハンドル検証に必須・未実装だと handle.invalid になる）
    - [x] Cloudflare DNS TXT によるハンドル検証（`_atproto.{handle}` TXT レコード自動管理）
    - [x] DID確定 → TXT セット → PLC送信 の順序保証（plc.directory イベント配信前に TXT を配置）
    - [x] PLC 送信リトライ時の genesis 再生成（同一署名での連続失敗を防止）
- [x] **4.3. ActivityPub 双方向フェデレーション完成**
  - [x] WebFinger エンドポイント（`GET /.well-known/webfinger`）をDBと接続し実データを返す（Content-Type: application/jrd+json 固定）
  - [x] Actor ドキュメント（`GET /users/:username`）をDBと接続し実データを返す
  - [x] NodeInfo エンドポイント（`GET /.well-known/nodeinfo` / `GET /nodeinfo/2.1`）の実装
  - [x] Inbox 受信処理: `Follow` / `Accept` / `Undo(Follow)` / `Create(Note)` のインライン処理実装
  - [x] フォロー関係 DB テーブル（`follows`）の `status` カラム追加（`pending` / `accepted`）
  - [x] こちら発のフォロー（`POST /api/follows/create`）: WebFinger → Actor取得 → Follow Activity送信 → pending状態で記録
  - [x] 投稿配送: 自アクターへの Follow 受理後、新規投稿を相手 Inbox へ HTTP Signatures 付きで配送
  - [x] Outbox エンドポイント（`GET /users/:username/outbox`）の実装
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
  - [x] `POST /api/follows/create` — Fediverse ユーザーへのフォロー送信
  - [x] `GET /api/users/profile` — ユーザープロフィール取得（ローカル / リモート統合）
  - [x] `GET /api/notes/:id` — ポスト詳細取得（フロントエンド向け JSON）
  - [x] `GET /notes/:id` — ActivityPub Note エンドポイント（Fediverse 向け、ローカルポストのみ）
    - nginx `map $http_accept` でコンテンツネゴシエーション（AP → api、ブラウザ → frontend）
- [x] **4.5.2. フロントエンド (React + Vite + TypeScript)**
  - [x] `frontend/` プロジェクト初期化
  - [x] ログイン画面 ・ ユーザー登録画面
  - [x] ローカルタイムライン画面（ローカル / ホーム タブ切り替え）
  - [x] 投稿入力フォーム
  - [x] ユーザープロフィール画面（`/profile?q=...`）: ローカル・リモートユーザー表示、フォローボタン
  - [x] タイムライン上のユーザー名クリック → プロフィール画面遷移
  - [x] ポスト詳細画面（`/notes/:id`）: 単一投稿表示、AP エントリーポイント兼用

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
- [ ] クォータチェック実装（ポスト送信・プロフィール保存時に実参照から計算）

### アップロード API
- [x] `POST /api/drive/files/create` — 画像アップロード（Misskey 互換）
  - オブジェクトストレージ未設定時は `503` を返す
  - ドライブ UI はフロントエンドに公開しない
- [ ] ポスト添付: `post_attachments` に保存（上限 4 枚、AT Protocol 互換）
- [ ] プロフィール更新 API にアバター・バナーの `media_file_id` 指定を追加

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
- [x] `GET/POST/DELETE /api/admin/emojis` — カスタム絵文字管理
  - shortcode の英数字・アンダースコアバリデーション、重複時 409 Conflict

### GC ジョブ
- [ ] 未参照かつ 7 日以上経過した `media_files` を定期削除するジョブ実装
  - object storage DELETE → `media_files` DELETE の順
  - `seiran-federation-worker` の job として組み込む

### SMTP 設定の管理画面化（`.env` からの移行）
- [ ] `site_settings` テーブル新規作成（キーバリュー形式 or 専用カラム）
  - SMTP: `smtp_host`, `smtp_port`, `smtp_username`, `smtp_password`, `smtp_from`
  - メール確認設定: `require_email_verification BOOLEAN DEFAULT false`
- [ ] 管理者 API: `GET/PATCH /api/admin/site-settings`
  - SMTP 設定はレスポンスでパスワードを返さない（`smtp_password_set: bool`）
- [ ] バックエンドの SMTP 送信を `.env` から DB 設定に切り替え
  - `mailer` は起動時固定ではなく、リクエスト時に DB から取得する構成に変更
  - SMTP 未設定時: メール送信が必要なエンドポイントは 503 を返す
- [ ] 新規ユーザー登録フローを `require_email_verification` に連動させる
  - OFF（デフォルト）: メール確認なしで直接登録できるフォームを表示
  - ON: SMTP 設定済みが前提。既存のメール確認フローを使う
  - フロントエンドは `GET /api/meta` に含まれる `requireEmailVerification: bool` を見てフォームを切り替える
- [ ] パスワードリセット機能も DB の SMTP 設定を使うよう更新
- [ ] `.env.example` から SMTP 設定項目を削除

> **実装方針**: パスワードリセット機能（フェーズ 2.8）は `.env` 設定で先行実装し、本タスクで DB 設定に切り替える。

---

## 🛡️ フェーズ5: 重複排除 (デデュプリケーション) ＆ マージエンジン (Deduplication)
マルチプロトコル間で生じる投稿の重複を、シナリオ別に水際で防ぐマージ・リンク機能を実装する。

- [ ] **5.1. シナリオ1: 自住民の投稿の逆輸入（ループバック）の検知・リンク**
  - [ ] ブリッジ等を経由して戻ってきた自サーバー住民の投稿を検知
  - [ ] `parent_original_post_id` に自サーバーのオリジナル投稿IDをハードリンク
- [ ] **5.2. シナリオ2: 他seiranユーザー間のマルチプロトコル投稿のマージ**
  - [ ] **送信側**: 投稿作成時に `seiran_post_uuid` を自動生成し、APのNoteカスタム拡張およびATP Postカスタムフィールドへ埋め込む処理の実装
  - [ ] **受信側**: 受信時に `seiran_post_uuid` をDB検索
    - [ ] 未登録なら新規インサート
    - [ ] 登録済みならインサートせず、既存の posts レコードに新着側のプロトコルID（`ap_object_id` または `at_uri`/`at_cid`）を `UPDATE` で追記書き込み
- [ ] **5.3. シナリオ3: 一般ブリッジユーザーの重複排除・リンク**
  - [ ] 外部ブリッジによる重複投稿を許容してDBに受け入れる
  - [ ] ブリッジメタデータの `source_url` 等をパースし、DB内にオリジナルが存在すれば `parent_original_post_id` に紐付ける

---

## 🔍 フェーズ6: 検索ステート ＆ セッションマネージャー (Search State)
メモリやRedisを活用した検索セッションのライフサイクル管理と、API互換ページネーション・ブレンドアルゴリズムを実装する。

- [ ] **6.1. `SessionStore` Trait とライフサイクル管理の実装**
  - [ ] `SearchSession` 構造体（クエリ、カーソル、未返却バッファ、最終アクセス時刻）の定義
  - [ ] **InMemorySessionStore**: `dashmap` を用いたスライディングタイムアウト（10分）とLRU追い出し（Eviction）ポリシーの実装
  - [ ] **RedisSessionStore**: JSONシリアライズと Redis TTLを用いたセッション管理の実装
- [ ] **6.2. 検索ページネーション・ブレンドアルゴリズムの実装**
  - [ ] **初回リクエスト**: ローカルDB ＆ AppView（`searchPosts`）の同時フェッチ、DB格納と統一ID付与、織り合わせマージ、未返却バッファとカーソルのセッション格納
  - [ ] **過去掘り (`untilId`)**: バッファ不足時のAppView追加フェッチ、ローカルDB追加フェッチ、再ブレンド、上位30件の返却
  - [ ] **未来掘り (`sinceId`) ＆ セッション消滅時**: ローカルDB限定検索への安全なフォールバック

---

## 🎨 フェーズ7: フロントエンド完成（3ペインUI） ＆ Misskey API互換レイヤー (Full UI & API)
フェーズ4.5のMVPを拡張し、独自Webフロントエンド（**React + Vite + TypeScript**）の
3ペインUIと Misskey API互換レイヤーを完成させる。

- [ ] **7.1. Misskey API互換エンドポイントの拡充 (バックエンド)**
  - [ ] `/api/notes/timeline`（ホームTL）、`/api/notes/search` 等の主要エンドポイントの実装
  - [ ] ページネーション（`sinceId` / `untilId`）を統一ポストIDと検索セッションにマッピングする処理
- [ ] **7.2. 拡張メタデータのAPIレスポンス埋め込み**
  - [ ] `bridge_real_actor_id`, `parent_original_post_id`, `seiran_pair_actor_id` などの拡張プロパティをレスポンスに付与
- [ ] **7.3. Webフロントエンド（React + Vite）の3ペインUIの実装**
  - [ ] デフォルトの3ペイン構造、ダイアログ駆動の画面遷移
  - [ ] 拡張メタデータの解析ロジック（アクターがブリッジか、魂の結合済みかどうかの判別）
  - [ ] UIの実装:
    - [ ] フォロー警告モーダル（ブリッジアカウントフォロー時の警告）
    - [ ] 「本尊ワープ」ボタン（ブリッジアカウントから本尊へシームレスにジャンプ）
- [ ] **7.4. サードパーティ製クライアント（ZonePane、Miria、Aria 等）向けの互換対応**
  - [ ] APIレスポンス送信時、アクターの `bio`（自己紹介）の末尾に本尊のURLを自動挿入するフォールバックロジックの実装
  - 注意: MiAuth ログイン互換対応（`/api/meta` 追加・check URL 修正）はフェーズ 2.4 で先行実施済み

---

## 🧪 フェーズ8: 統合テスト ＆ 最適化 (Test & QA)
システム全体の信頼性、高負荷時の挙動、および物理分割の検証を行う。

- [ ] **8.1. 単体・結合テストの作成**
  - [ ] 重複排除（シナリオ2のマージ処理）のユニットテスト
  - [ ] 未来補正タイムスタンプの採番テスト
  - [ ] 検索ブレンドアルゴリズムの挙動テスト
- [ ] **8.2. 連合（Federation）統合テスト**
  - [ ] モックAPサーバー、モックATPサーバーを用いた連携テスト
  - [ ] リモートseiran同士のハンドシェイクおよび特権同期のテスト
- [ ] **8.3. 高負荷・スケールアウト検証**
  - [ ] `AUTH_PROVIDER=auth0` ＋ `RedisJobQueue` ＋ `RedisSessionStore` を用いた環境での動作確認
  - [ ] プロダクションビルド・デプロイ手順の検証
