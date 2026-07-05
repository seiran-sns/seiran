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
    - [x] `commit_record_inner` DB 操作 4 本をトランザクション化（atp_blocks INSERT・actors UPDATE・atp_records INSERT・atp_repo_events INSERT をひとつのトランザクションに束ね、WebSocket ブロードキャストを commit 後に実行）
    - [x] `atp_repo_events.frame_bytes` カラム追加（zstd 圧縮）: commit 時生成フレームを保存し cursor 再送で再構築せずそのまま送出（再構築フレームと commit 時フレームのバイト列差異による Relay 切断を解消）
    - [x] `#identity` フレーム実装（`atp_repo_events.event_type` 列追加で commit/identity を同テーブル管理）: ユーザー登録完了後に Relay へ handle 再検証を促す。起動時に identity イベント未送信の既存ユーザーへ自動補完。
    - [x] 画像 embed の DAG-CBOR 形式修正: blob ref は `{"$link": tag42}` ではなく `Ipld::Link(cid)` = tag42 直接（AT Protocol CBOR 仕様準拠）
    - [x] `#commit` フレームの `blobs` フィールド実装: 新規コミットで参照する blob CID リストを含める（空だと AppView が画像投稿をインデックスしない）
    - [x] `com.atproto.sync.getBlob` 実装: sha256 で `media_files` を検索し CDN URL へ 307 リダイレクト
  - [x] Bsky 向けメンション変換の基礎実装（ローカル `@user` → `@user.{domain}`、Fedi リモート → brid.gy 2段階ルックアップ）
  - [x] Bsky 向けメンション Facet 生成（AT Protocol RichText facets への変換。現状はテキスト置換のみ）
  - [x] **リポスト重複制約**: `UNIQUE INDEX (actor_id, repost_of_post_id) WHERE deleted_at IS NULL` をマイグレーションで追加（取り消し前の再リポストを DB レベルで禁止）
  - [x] **リポストのクロスプロトコル配信**（spec doc 9.1〜9.3 節参照）
    - [x] Fedi リモートポストのリポスト → Bsky: DB の at_uri 確認 → フォールバック URL カード投稿（初期実装: ブリッジ探索なし）
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
  - [x] `POST /api/follows/create` — Fediverse ユーザーへのフォロー送信
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
  - [x] 投稿入力フォーム
  - [x] ユーザープロフィール画面（`/profile?q=...`）: ローカル・リモートユーザー表示、フォローボタン
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
    - 右ペインの動的コンテキスト切替（Doc5 §2）: ホーム=トレンド＆検索/通知、プロフィール=投稿リスト全面、ポスト詳細=投稿主前後/リアクション
    - `RightPaneContext` でサブタブインデックスをセッション内保持（Doc5 §2.4）
  - [x] 拡張メタデータの解析ロジック（アクターがブリッジか、魂の結合済みかどうかの判別）
    - `ProfileResponse` に `bridge_real_handle` / `bridge_protocol` / `is_paired` / `at_did` / `bio` を追加
    - `NoteCard` が `renoteId` / `quoteId` / `replyId` / `parentOriginalId` を解釈してバッジ・導線を描画
    - [x] リポストのカード表示（#45）: `NoteResponse.renote` に元ポストを埋め込み、「（表示名）が（日時）にリポスト」ヘッダ＋元ポスト本体を描画。日時はリポスト自身の詳細へリンク
    - [x] リポストボタン: `NoteCard` に 🔁 ボタンを追加。`api.notes.create({renote_id})` でリポスト作成、StreamHub 経由でリアルタイムTL反映
    - [x] AP 受信リポスト（Announce）: `handle_announce()` で元ポストを DB から検索し `repost_of_post_id` 付きで保存。`Undo(Announce)` で論理削除
  - [x] UIの実装:
    - [x] フォロー警告モーダル（ブリッジアカウントフォロー時の警告・Doc5 §3.2）
    - [x] 「本尊ワープ」ボタン（ブリッジアカウントから本尊へシームレスにジャンプ・Doc5 §3.1）
  - 注: トレンド集計・通知・絵文字リアクションはバックエンド未実装のためプレースホルダ表示（実装後に有効化）
- [x] **7.3.x. 退会機能 Phase A（#29）**
  - [x] `actors.withdrawn_at` カラム追加（マイグレーション）
  - [x] `POST /api/account/withdraw`（`confirm_handle` 必須、AP Delete(Actor)配送 + ATP #account broadcast + 全投稿論理削除 + `withdrawn_at` セット）
  - [x] ATP subscribeRepos `#account` フレーム生成（`build_account_frame`）+ `broadcast_account_event`
  - [x] AP `deliver_delete_actor` — Fedi フォロワー全員に Delete(Actor) 送信
  - [x] フロントエンド: プロフィール編集画面の下部に「退会」セクション（ハンドル確認入力必須）
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
