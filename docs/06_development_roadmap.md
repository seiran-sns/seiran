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
外部のAuth0および内蔵のローカル認証を切り替え可能にし、Misskeyクライアント向けのMiAuthエンドポイントを実装する。

- [x] **2.1. `AuthProvider` Trait (インターフェース) の定義**
  - [x] `verify_token` メソッドの定義と、`ExtUserInfo` 構造体の設計
- [x] **2.2. Auth0 プロバイダ実装 (`AUTH_PROVIDER=auth0`)**
  - [x] Auth0 SDK/JWTデコードを用いた外部トークンの検証とユーザーマッピング
- [x] **2.3. ローカル認証プロバイダ実装 (`AUTH_PROVIDER=local`)**
  - [x] 自前DB（`users`等）との連携、Argon2によるパスワードハッシュ検証
  - [x] SMTPモジュールによる新規登録時のメール確認機能の枠組み実装
- [x] **2.4. MiAuth (Misskey認証) 互換エンドポイントの実装**
  - [x] `/miauth/authorize` エンドポイントの実装
  - [x] Auth0等のセッションと紐付けたアクセストークン発行・偽装ロジックの実装
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

---

## 🌐 フェーズ4: マルチプロトコル通信エンジン ＆ ゼロトラストペアリング (Federation Engine)
ActivityPubおよびAT Protocolのプロトコルレベルの同期・解決、そしてリモートseiranユーザー同士の直接ペアリングを実装する。

- [x] **4.1. ActivityPub (Fediverse) 統合**
  - [x] Webfinger解決、Inbox（受取）および Outbox（過去ログ配送）ハンドラの実装
  - [x] HTTP Signaturesによる署名検証、公開鍵キャッシュ
  - [x] Outbox同期時の「過去30日間 / 最大300件」キャップ処理 (ベストエフォート)
- [ ] **4.2. AT Protocol (Bluesky) 統合**
  - [x] DID解決（`did:plc`, `did:web`）、AppView APIクライアントの実装
  - [x] `getAuthorFeed` を用いた外部アクターの過去ログフェッチ（過去30日間 / 最大300件キャップ）
  - [x] Bluesky Firehose 受信モジュールの実装（外部アクターの新着ポストをリアルタイム DB 保存）
  - [ ] seiran PDS としてのローカルユーザー ATP リポジトリ管理（MST署名コミット + Relay 配信）
    - [ ] ユーザー登録時の `did:plc` 発行（plc.directory への登録）
    - [ ] 投稿時の DAG-CBOR エンコード + MST コミット + P-256 署名
    - [ ] `com.atproto.sync` 経由での Relay へのブロードキャスト
- [ ] **4.3. リモート seiran アクター専用ハンドシェイク (ゼロトラスト検証)**
  - [ ] Bioの `seiran_signature: [ATP_DID]` パターン検出ロジックの実装
  - [ ] 相手ドメインの `/.well-known/seiran/verify-actor` への検証リクエストの実装
  - [ ] 検証成功時の `actor_type = 'remote_seiran'` 昇格と `seiran_pair_actor_id` の相互紐付け
- [ ] **4.4. リモート seiran 特権初期同期**
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
  - [x] `POST /api/notes/create` — ローカル投稿作成
  - [x] `GET /api/notes/local-timeline` — ローカルタイムライン（`sinceId` / `untilId` ページネーション）
- [x] **4.5.2. フロントエンド (React + Vite + TypeScript)**
  - [x] `frontend/` プロジェクト初期化
  - [x] ログイン画面 ・ ユーザー登録画面
  - [x] ローカルタイムライン画面
  - [x] 投稿入力フォーム

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
