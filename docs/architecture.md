# アーキテクチャ

対象読者: seiran のコード全体に手を入れる開発者。「今のシステムがどう動いているか」だけを書く。変更の経緯や過去の不具合修正は書かない（必要なら `git log` を見る）。

## 1. プロトコル上の位置づけ

seiran は Fediverse (ActivityPub) と Bluesky (AT Protocol) の両方に**サーバーとして参加する**。

- **AP側**: 一般的な Fedi インスタンスと同じく、Actor・Inbox・Outbox・WebFinger を自前で持つ。
- **ATP側**: 外部 PDS（bsky.social 等）を使わず、**seiran 自身が各ローカルユーザーの PDS（Personal Data Server）を兼ねる**。ユーザーごとに `did:plc` を発行し、投稿のたびに自前で MST（Merkle Search Tree）をコミット・P-256 署名し、公式 Relay（`bsky.network`）へ配信する。AppView（bsky の検索・フィード生成)は Bluesky 公式のものをそのまま利用し、seiran はそこに投稿を流し込む立場。

この非対称性（AP はクライアント兼サーバー的にフラットだが、ATP は「PDSを自作している」）が実装の複雑さの主な発生源になっている。

## 2. ワークスペース構成

`Cargo.toml` の workspace members は6 crate。ビルド成果物は `seiran-server` の**単一バイナリのみ**で、他はすべて lib crate。

| crate | 種別 | 役割 |
|---|---|---|
| `seiran-common` | lib | 全crate共通の基盤。DB・認証・シークレット管理・ジョブキュー/ジョブハンドラ・AP/ATPクライアント・Repository層・ストレージ・ストリーミングハブ |
| `seiran-api` | lib | Web API 本体。Misskey互換API、MiAuth、XRPC(AT Protocol)、drive(メディア)、admin API。axum Router と `AppState` |
| `seiran-federation-inbox` | lib | ActivityPub 受信ゲートウェイ。inbox・webfinger・actor・outbox・nodeinfo・featured/lists の公開エンドポイント |
| `seiran-federation-worker` | lib | ジョブキューをデキューして実行するワーカーエンジンの起動処理のみ。ジョブの実処理は `seiran-common::jobs` にある |
| `seiran-atp-repo` | lib | Bluesky Jetstream を購読し、フォロー済みDIDの新着投稿/Likeを取り込むリスナー |
| `seiran-server` | bin | 唯一の実行バイナリ。`--role` で上記各lib crateを配線して起動する |

`seiran-common` の主要モジュール:
- `auth/local.rs` — ローカル認証（Argon2 + JWT）
- `secrets.rs` — `secrets.toml` 自動生成
- `queue/` — `JobQueue` の InMemory/Redis 実装とワーカーエンジン
- `jobs/` — 各ジョブの実処理（`ap_delivery`, `atp_repository_publish`, `inbound_activity_process` 等）
- `ap/` — ActivityPub クライアント・配送・webfinger・outbox
- `atp/` — MST/リポジトリ、PLC、DID解決、service auth
- `repository/` — Repository パターンの実装群（`actor.rs`/`post.rs`/`follow.rs` 等）
- `storage/` — S3互換クライアント、ストレージ選択、画像処理
- `streaming.rs` — `StreamHub`（WebSocket配信）
- `id.rs` — Snowflake ID 採番
- `jetstream_control.rs` / `jetstream_leader.rs` — Jetstream 接続のプロセス間調整

## 3. 統合バイナリとロール分割

`seiran-server/src/main.rs` の `Role::resolve()` が `--role=xxx` → `SEIRAN_ROLE` 環境変数 → 未指定なら `All` の順で解決する。

| CLI値 | Role | 対応crate | ポート |
|---|---|---|---|
| `all`(既定) | All | 全部合流 | `PORT`(既定3000) |
| `api` | Api | seiran-api | `PORT` |
| `federation` / `inbox` | Federation | seiran-federation-inbox | `FEDERATION_INBOX_PORT`(既定3001) |
| `worker` | Worker | seiran-federation-worker | なし |
| `firehose` / `atp-repo` | Firehose | seiran-atp-repo | なし |

- **`Role::All`**: DB接続・シークレット・HTTPクライアント・`job_queue`（常に InMemory）を1回だけ生成し、`seiran_api::router().merge(seiran_federation_inbox::router())` で単一 axum Router として1ポートで待ち受ける。firehose と worker は同一プロセス内で `tokio::spawn` されるバックグラウンドタスク。
- **`Role::Api` / `Role::Federation`**: 単独プロセスとして専用ポートで待受。`REDIS_URL` があれば `RedisJobQueue`、なければ `InMemoryJobQueue`（split-role構成でこれを選ぶと他プロセスにジョブが届かない）。
- **`Role::Worker`**: HTTPサーバーは立てず、ジョブキューを消費するのみ。
- **`Role::Firehose`**: 購読者がいないため空の `StreamHub` を使う。

同じ Docker イメージを `command`（`--role`）違いで複数コンテナに分けるか、単一コンテナで `all` 起動するかは**運用モードの選択**であり、コード上の分岐は `main.rs` の `Role` 列挙とその配線だけ。

- `docker-compose.yml`（split-role）: `db` / `redis`（ジョブキュー共有に必須）/ `api` / `federation-inbox` / `worker` / `atp-repo` / `frontend` / `nginx`（`docker/nginx.conf`）/ `tunnel`。`config-data` ボリュームで `secrets.toml` を全バックエンド間で共有永続化する。
- `docker-compose.mono.yml`（単一コンテナ）: `db` / `seiran-server`（role=all）/ `frontend` / `nginx`（`docker/nginx.mono.conf`）/ `docker-gen`（`--scale seiran-server=N` によるスケールアウト時に nginx へ反映）/ `tunnel`。Redis サービス自体が存在しない（同一プロセス内で完結するため不要）。

## 4. 認証

認証はローカル ID/PW のみ（`seiran-common::auth::local::LocalAuthProvider`）。外部認証プロバイダとの連携や、認証方式を切り替える抽象化レイヤーは存在しない。

- パスワード: Argon2（`argon2` クレート既定パラメータ、`OsRng` で salt生成）
- トークン: `jsonwebtoken` による JWT（HS256相当）。`sub` は `"local|{user_id}"`、有効期限7日。secret は `secrets.toml` の `jwt_secret`（256bit hex、起動時自動生成）。

**MiAuth 互換**（Misskeyクライアント向け）: `GET /miauth/:session_id`（認可ページ）→ `POST /api/miauth/:session_id/authorize`（要Bearer、認可するとそのユーザーのJWTを発行）→ `POST /api/miauth/:session_id/check`（クライアントがポーリングして受け取る）。セッション状態は `AppState.miauth_sessions`（プロセス内メモリ、DB永続化なし）。

**Misskey API 互換との共存**: `middleware::misskey_auth_bridge` が、Misskeyクライアントが送る JSON ボディの `i` フィールドまたはクエリの `i` を検出して `Authorization: Bearer` ヘッダーへ合成する（既存の `Authorization` ヘッダーがあればそちらを優先）。つまり JWT ベースのローカル認証が唯一の実体で、MiAuth と Misskey 互換はその上に被さる「トークンの発行・受け渡し窓口」に過ぎない。multipart/form-data のボディ（`drive/files/create` のファイルアップロード）はこのミドルウェアの対象外のため、`handlers::drive::create_drive_file` はハンドラ内で multipart の `i` フィールドを個別にフォールバックとして扱う。

### API エラーレスポンス方針
`ApiError` は `{"code": "ERROR_CODE"}` 形式の JSON を返す（平文テキストは返さない）。Misskey 互換エンドポイントでは追加で `error: {code, message}` も付与し後方互換を保つ（`message` は常に `code` と同一文字列で、人間可読なメッセージ生成はフロントエンドの責務）。エラーコードはフロントエンドの `client.ts` の `getErrorMessage()` が `i18n/locales/{lng}/errors.json` の翻訳へマップする（未知のコードは HTTP ステータスが5xxなら「サーバー応答なし」文言に、それ以外はコード付きの汎用文言にフォールバック）。トークン失効（401、かつローカルにトークン保持中）を検知すると `setUnauthorizedHandler()` 経由で `AuthProvider` へ通知し、自動ログアウト＋ログイン画面誘導を行う。

## 5. ジョブキュー

`seiran-common::traits` に `Job`（enum）、`JobQueue` trait（`enqueue`/`enqueue_retry`/`dequeue_blocking` の3メソッドのみ）を定義。`WorkerEngine` はこの trait のみに依存しバックエンド実装を意識しない。

**バックエンド選択**（`create_job_queue(is_monolith: bool)`）:
- `is_monolith == true`（`--role all`）: 常に `InMemoryJobQueue`（`REDIS_URL` の有無を見ない）
- `is_monolith == false`（split-role）: `REDIS_URL` があれば `RedisJobQueue`（優先度付き Sorted Set + `BZPOPMIN` + Lua スクリプトによる遅延リトライ昇格）、なければ `InMemoryJobQueue` にフォールバック

**主要ジョブ**:
| Job | 用途 | 優先度 |
|---|---|---|
| `ActorHistorySync` | 新規フォロー時の過去ログ取得（最大300件） | 低 |
| `ApDelivery{actor_id, kind}` | AP配送。`kind` は `PostToFollowers`/`DirectMessage`/`Announce`/`UndoAnnounce`/`DeleteNote`/`Reaction`/`UndoReaction`/`UpdateActor`/`DeleteActor`（`DirectMessage`はDM宛先個人のみへの配送、`docs/protocols.md` 9節） | 高 |
| `InboundActivityProcess` | 受信AP活動の非同期解析・DB保存（inboxハンドラは署名検証のみ同期実行し即202を返す） | 中 |
| `ActorMetadataResolve` | リモートアクター検証・メタデータ取得 | — （**スタブのみ、enqueueする箇所が実装されていない**） |
| `AtpRepositoryPublish` | 外部PDSへのミラーリング目的で定義されているが、**enqueueする呼び出し箇所が実質存在しない**（現在の投稿配送は `AtpCommitService` を直接 await する経路に一本化されている。デッドコード） | 最高 |
| `BskyVideoPoll` | Bsky公式動画パイプラインの完了ポーリング | — |
| `ProxyFollowSync` | list-relay仮想アクターの代理フォロー同期 | — |
| `AccountWithdrawUnfollowAll` | 退会時の一括アンフォロー | — |
| `BskyPostCommitDeferred` | 動画添付投稿のATPコミットを動画結合完了まで遅延 | — |
| `ResolveBskyMention{did}` | Bskyメンションfacetの未知DIDをAppViewから先行解決し`actors`へupsert（`docs/protocols.md` 6節）。ベストエフォート、表示時にも都度解決するため必須ではない | 中 |
| `BskyDmSend{post_id}` | DM宛先のBskyアクターへ`chat.bsky.convo.sendMessage`で送信（`docs/protocols.md` 9節） | 高 |

**並列・排他制御**: グローバル同時実行数上限（`Semaphore`、既定32）、ドメイン単位の同時接続数制限（最大2並列、AP配送用）、アクターID単位の直列化（ATPコミットの順序保証）、指数バックオフ+ジッターでのリトライ。

## 6. 検索セッション管理

HTTP はステートレスであり、フロントエンドが検索画面をいつ閉じたかバックエンドは検知できない。そこでメモリ（将来はRedis）上に「10分間の砂時計」としてセッションを持つ。

```rust
pub struct SearchSession {
    pub query: String,
    pub appview_cursor: Option<String>,          // AppViewの次回カーソル
    pub unreturned_appview_posts: Vec<Post>,      // 取得済み未返却バッファ
    pub last_accessed_at: DateTime<Utc>,          // 寿命延長の主軸
    pub appview_exhausted: bool,
}
```

- **寿命**: 10分のスライディングタイムアウト。アクセスのたびに延長。
- **保存先の抽象化**: `SessionStore` trait。現状は `InMemorySessionStore`（`dashmap`）のみ実装。`RedisSessionStore` は未実装（`docs/roadmap.md` フェーズ8参照）。

**ブレンドアルゴリズム**（Misskey API互換の ID ベース要求 ⇄ AppView のカーソルベース要求を翻訳する）:
1. **初回検索**: ローカルDB検索とAppView検索(`app.bsky.feed.searchPosts`)を `tokio::join!` で同時フェッチ、それぞれ30件。AppView分はローカルDBにインサートし統一IDを付与してから、統一ポストIDの降順でマージし上位30件を返却。残りはバッファとしてセッションに保存。
2. **過去掘り**（`untilId`）: バッファが `limit` 未満ならAppViewへ追加フェッチ。ローカルDBからも追加取得し再ブレンド。
3. **未来掘り**（`sinceId`）: AppViewへは問い合わせず、**ローカルDB検索のみ**で完結（過去に通過したAppView投稿は既にローカルDBにインサート済みのため取りこぼしがない）。
4. **セッション消滅時**: エラーを返さず、通常のローカルDB検索へ自動フォールバックしベストエフォートで結果を返す。

ブレンド処理の中核（ID列のマージ・降順ソート・重複排除・`limit`件での返却分/バッファ分への分割）は `handlers/search.rs` の `merge_sort_dedup_and_split()` として `AppState`（DB・HTTPクライアント）に依存しない純粋関数に切り出されており、単体テストで複数ページ・重複IDのシナリオを検証している。`InMemorySearchStore`（`search.rs`）の `create`/`take_buffer`/`put_buffer`/`cleanup` も同様に単体テスト済み。

## 7. ストレージ・シークレット管理

**secrets.toml 自動生成**（`seiran-common::secrets`）: `SEIRAN_CONFIG_DIR`（既定 `./config`）配下の `secrets.toml` を読み、無ければ生成してパーミッション0600で保存。含まれるもの:
- `jwt_secret`（256bit hex）
- AT Protocol 用 P-256 鍵ペア
- AP HTTP Signatures 用 RSA-2048 鍵ペア
- `encryption_key`（AES-256-GCM、DB内の機密フィールド暗号化用）

`storage_providers.secret_key` 等は `encryption_key` で AES-256-GCM 暗号化して DB に格納する（`crypto.rs`）。

**S3互換オブジェクトストレージ**: `storage/selector.rs` の `select_provider()` が有効なプロバイダーを id 昇順でスキャンし、`capacity_mb` に収まる最初の1件を選択する（複数プロバイダーの容量切り替え）。`storage/s3.rs` が実際の PUT/DELETE、`media_probe.rs` が動画音声のプローブを担う。

**画像アップロードパイプライン**（`storage/image.rs::prepare_image()`）: ユーザーの画像を不要に劣化させないため、2つの候補を用意してから採用する。まず `storage/exif.rs`（`img-parts`クレート使用）でJPEG/PNGのExifをOrientationタグのみに絞り込んだ「無劣化オリジナル候補」を作る（画素は再エンコードしない）。続けてOrientationを画素に適用したうえで `MediaKind` ごとの最大サイズにリサイズしWebPロスレスエンコードした「リサイズ候補」を作る。呼び出し元（`handlers/media_store.rs::store_image()`）が両候補それぞれのsha256+blurhashで `media_files` の重複排除チェックを行い、どちらも未登録ならバイトサイズが小さい方を採用してS3へアップロードする。img-parts非対応フォーマット（静止画WebP・AVIF・単一フレームGIF等）はOrientation適用のみ行いWebP再エンコードする（オリジナル候補なし）。アニメーション画像（GIF/APNG/WebPアニメ）は元バイト列をそのまま保存する。

## 8. フロントエンド

React 18 + Vite + TypeScript（react-router-dom v6）。`frontend/src/` 構成:

- `api/client.ts` — バックエンドAPIクライアント、`ApiError`、`getErrorMessage()`
- `components/layout/` — `AppShell`（3ペインの外枠）、`LeftNav`
- `components/note/` — `NoteCard`（タイムライン・詳細・プロフィール共通の投稿カード）、`PostComposer`、`ReactionChips`（各チップのホバーでリアクター一覧をポップオーバー表示、`ReplyIndicator`と同じ遅延フェッチ・遅延クローズパターン）/`ReactionPicker`（トリガーボタン＋`Modal`内の`EmojiPickerPanel`。Unicode絵文字データセット（`unicode-emoji-json`）は`React.lazy`で遅延ロードし、カスタム絵文字とあわせて検索・タブ切り替えで選べる）、`HlsVideo`、`RichText`（本文中のMarkdownリンク`[text](url)`・生URL・`@mention`・`#ハッシュタグ`・絵文字ショートコードを1パスでクリック可能な要素へ変換。AP由来のハッシュタグアンカー`[#foo](リモートURL)`もリンクテキストの形状で検出し自インスタンスの`/tags/foo`へ読み替える。`EmojiText`は表示名等リンク化不要な箇所向けにショートコード置換のみ残す）等
- `components/right/` — 右ペインのタブ内容（`NotificationsPanel`、`TrendsSearchPanel`）
- `components/admin/` — 管理画面パネル群
- `components/dm/` — `RecipientPicker`（DM宛先のchip入力。サジェスト選択/手打ち確定の両対応、Bskyアクターと他プロトコルの混在を警告表示）
- `contexts/` — `AuthContext`、`ComposerContext`（返信モーダルに加え、`openCompose(initialText)` で本文プリフィル済みの素の投稿モーダルもグローバルに開ける）、`RightPaneContext`（右ペインのサブタブ状態保持）、`StreamingContext`（WebSocket集約。DM新着（`visibility=direct`のnoteイベント）は`registerDirectMessage`で別系統に振り分け、未読セッション数`dmUnreadCount`をLeftNavのバッジに供給する。Fediフォロー承認（`followAccepted`）受信時は`stores/followStatusStore`を直接更新する、`docs/protocols.md` 8節）、`ToastContext`（エラー/成功/情報トースト通知）、`NavigationHistoryContext`（`useGoBack()`。SPA内でPUSHされたナビゲーションの深さを追跡し、各画面の「戻る」ボタンから共通で使う。直接URLを踏む・リロードする等でSPA内に戻り先が無い場合は`navigate(-1)`の代わりにホーム（`/`）へ遷移する）
- `stores/followStatusStore.ts` — フォロー状態（`not_following`/`pending`/`accepted`）の外部ストア（Reactコンテキストではなくモジュールスコープの`Map`+`useSyncExternalStore`）。キーは`lib/format.ts`の`profileQuery(username, domain)`で統一。同一アクターのフォロー状態表示は画面内に複数存在しうる（プロフィール本体、右ペインのポストリスト、タイムライン上の同一ユーザーの複数投稿）ため、各コンポーネントがローカルstateで抱えず全てこのストアを購読する設計にし、一箇所の操作（フォロー/フォロー解除ボタン）・WebSocket経由の`followAccepted`受信のいずれでも表示中の全コンポーネントへ同時反映する。`ProfilePage`のフォローボタン、`NoteCard`のタイムライン上のフォロースイッチが利用
- `pages/` — 画面単位のトップレベルコンポーネント（`MessagesPage`はDM専用画面、`docs/ui_spec.md`参照）
- `i18n/` — 国際化。`react-i18next` + `i18next-browser-languagedetector`。「自動」時はブラウザの言語設定に従い判定（`en`/`ja`、対応言語外は `en` にフォールバック）。設定画面「表示」（`/settings/appearance`、#55）でユーザーが明示的に言語を選択した場合は`localStorage`に記憶（`detection.caches`）し、ログイン中はさらに`users.language_preference`（サーバー保存値）が優先される（`AuthContext`がログイン/`GET /api/auth/me`取得時に`i18n.changeLanguage()`を適用）。翻訳リソースは `i18n/locales/{en,ja}/{namespace}.json` に画面・機能単位の名前空間で分割配置し、`i18n/index.ts` の `resources` に集約してビルド時にバンドルする。名前空間分割は、将来ユーザーが独自の言語ファイル（同形式のJSON）を作成・適用・配布できるようにする構想を見据えたもので、`i18n.addResourceBundle()` により実行時にリソースを追加・上書きできる

3ペインUIのレイアウト仕様は `docs/ui_spec.md` を参照。

**開発用プロキシとVite内部パスの衝突**: `frontend/vite.config.ts` の開発サーバー（ローカル `cargo run` 直接起動時のみ有効）は `GET /@:handle`（プロフィールページ）をバックエンドへ転送するが、単純なプレフィックスマッチだとVite自身の内部モジュール（`/@vite/client`・`/@react-refresh`・`/@fs/...`・`/@id/...`）まで巻き込んでバックエンドへ転送してしまい、Viteクライアントが読み込めず白画面になる（実機確認）。そのためこれらを除外する正規表現（`^`始まりはVite側でregex扱い）を使う。

## 8.1 OGP (Open Graph) 対応

フロントエンドは SPA のため、素の index.html には投稿・プロフィールごとの `<meta>` が無い。
User-Agent で既知の bot だけを判定して出し分ける方式は、リストにない未知のクローラーを
取りこぼすため採用していない。代わりに `/notes/:id`・`/@:handle`（AP クライアント向け
`Accept` を除く）へのリクエストは常に、SPA の index.html の `<head>` に OGP `<meta>` を
注入したものを返す。実ブラウザはそのまま SPA が起動し（`<meta>` 注入以外は普段と同じ
index.html）、クローラーは JS を実行しないため `<meta>` だけを読んで終わる。

- `crates/seiran-api/src/handlers/ogp.rs` — DB から投稿/アクターの情報を取得し、
  `state.frontend_origin`（Vite dev server、Docker既定は`http://frontend:5173`、環境変数
  `FRONTEND_ORIGIN`で上書き可）から index.html を取得して `<title>`・OGP/Twitter Card の
  `<meta>` を注入する。`GET /notes/:id` は既存の AP Note エンドポイント（`get_note_ap`）が
  `Accept` ヘッダーで分岐し、AP クライアント向け JSON-LD とこの OGP 注入 HTML を出し分ける。
  `GET /@:handle` はプロフィール専用の新規エンドポイント。
- 投稿・アクターが見つからない/DBエラー時は `<meta>` を注入せず index.html をそのまま返す
  （ここで 404 等を返すと SPA 自体が起動できず、フロント側の「見つかりません」表示や
  リモートアクターの都度フェッチが機能しなくなるため）。
- 可視性は投稿・プロフィールいずれも通常の閲覧経路と同じ判定を通す（`followers_only`/
  `direct` は非表示、`PostRepository::find_by_id_for_viewer` を viewer なしで呼ぶ）。
- nginx（`docker/nginx.conf`/`docker/nginx.mono.conf`）・ローカル開発（`frontend/vite.config.ts`
  の proxy）とも、`/notes`・`/@` は bot 判定なしで常に api（バックエンド）へ転送する。

## 9. E2Eテスト

`e2e/`ディレクトリにPlaywrightプロジェクトを置く。外部の実サービス（fedi/Bskyインスタンス、PLCディレクトリ、Bsky Relay等）とは通信せず、seiranが話す相手をすべてローカルのスタブ/専用インスタンスに置き換えた上で実行する。実行は `cd e2e && npm test`。

- `e2e/playwright.config.ts`: `webServer`にスタブPLCサーバー・スタブAppViewサーバー・スタブFediサーバー（`stub-fedi-server.ts`、後述）・backend（`cargo run -p seiran-server`）・frontend（`npm run dev`）をまとめて起動する。backendには`PLC_DIRECTORY_BASE_URL`/`ATP_APPVIEW_URL`をそれぞれのスタブサーバーへ、`ATP_RELAY_URL`を存在しないローカルポートへ向け、`CLOUDFLARE_API_TOKEN`/`CLOUDFLARE_ZONE_ID`を空文字にして、外部への実通信を確実に遮断している。`SQLX_OFFLINE=true`も設定し、マイグレーション未適用の空DBに対してsqlxのコンパイル時クエリ検証が失敗しないようコミット済み`.sqlx/`キャッシュを使わせる。
  - 【重要】全`webServer`エントリの`reuseExistingServer`は`false`固定（変更禁止）。backendPort(3000)/frontendPort(5173)は`scripts/dev-up.sh`のネイティブ開発サーバーとも共有しており、`true`だと起動中の実開発サーバーへ無条件に相乗りしてしまう。2026-07-20に実際に発生し、実開発DBへのテストデータ混入・本物のplc.directoryへの誤登録という事故になった（後者は`did:plc:`のtombstoneオペレーションで収束済み）。`false`ならポート競合時に明確なエラーで停止する。
  - 【重要】Playwrightの実行順序は直感に反して「webServer起動 → globalSetup」（`globalSetup`ではwebServerの起動には間に合わない）。そのためE2E専用Postgres（`e2e/docker-compose.yml`、ポート5433）の起動待ちは`globalSetup`ではなく`e2e/scripts/wait-for-db.ts`としてbackendの`command`自体の前段に組み込んでいる。逆に`e2e/global-setup.ts`は「backendが起動済み」を前提にできるので、初期管理者アカウントのbootstrapに使っている（`GET /api/setup/status`は`users`テーブルが1件でもあれば`initialized:true`を返し、未初期化だとフロントは`App.tsx`のルーティングを無視して常に`<Setup>`画面を表示するため、E2E専用DBは空の状態からテストを始める都合上これが必要）。`globalTeardown`はE2E専用Postgresを`down -v`で破棄する。
- `e2e/fixtures/stub-plc-server.ts`: `plc.directory`のスタブ実装。TypeScript、Node.js（v22+のネイティブTS実行を利用しビルド不要）。genesis opを受け取ってメモリに保持し、GET時にDIDドキュメント形式へ組み直して返す。
- `e2e/fixtures/stub-appview-server.ts`: Bsky AppView（`public.api.bsky.app`）のスタブ実装。`app.bsky.feed.searchPosts`等の主要エンドポイントに対し常に空の結果を返す（seiranのローカルDB検索はこれと独立して機能するため、ローカル投稿の検索はAppViewが空でも成立する）。
- `e2e/fixtures/stub-fedi-server.ts`: リモートのActivityPubアクター（Mastodon等）のスタブ実装。正規のHTTP Signatures（RSA-SHA256、Digestヘッダー必須。`crates/seiran-common/src/ap/client.rs`のcanonical signing string規約に準拠）で署名したFollowをseiranの`/inbox`へ送り、フォロー成立後の投稿・返信・リポスト配送（Fedi配送はローカルアクターのacceptedフォロワー全員へのファンアウトのみで、返信先個人への直接配送やsharedInboxは無い。`crates/seiran-common/src/ap/deliver.rs`）を自身のinboxで受信・記録できる。
- `e2e/fixtures/api-helpers.ts`: テスト対象でないセットアップ（フォロー相手ユーザーの作成等）はUI操作ではなく`/api/auth/register`を直接叩いて済ませ、各テストは検証したいUI操作に集中させる。ログイン状態が前提のテストは`seedAuth()`でlocalStorageにtokenを仕込みUIログイン操作自体を省略できる（ログインフロー自体を検証する`login.spec.ts`だけは実際にフォームを操作する）。
- テストファイルは`e2e/tests/`配下（`signup`・`login`・`post`・`follow`・`reply`・`reaction`・`search`・`profile-edit`・`hashtag`・`federation-delivery`）。DBはテスト実行全体で共有されるため、各テストはユーザー名が衝突しないよう一意なプレフィックス+タイムスタンプで登録する。
- フロントは`i18next-browser-languagedetector`がブラウザロケールを見て言語を決めるため、Playwright側は`use.locale`を`ja-JP`に固定している（既定の`en-US`だとUIが英語化される）。
- DBはE2E専用インスタンスを使い、テスト実行のたびに空の状態から始める（アカウントは各テストが必要に応じて新規作成する）。手動検証用の`seiran{n}`アカウント（本ファイル冒頭のCLAUDE.md参照）とは分離されている。
- Cloudflare DNS（ATPハンドル検証のTXT自動登録）、通知UI（未実装）はE2Eのスコープ外。
- GitHub Actionsへの組み込みは未対応（当面はローカル実行のみ）。

## 10. 環境変数

| カテゴリ | 変数 |
|---|---|
| ドメイン | `LOCAL_DOMAIN`, `ATP_PDS_ORIGIN` |
| 起動ポート | `PORT`(既定3000), `FEDERATION_INBOX_PORT`(既定3001) |
| データベース | `POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB`, `DATABASE_URL` |
| ジョブキュー | `REDIS_URL`（split-role構成専用。`--role all` では不要） |
| シークレット | `SEIRAN_CONFIG_DIR`（既定 `./config`）。JWTシークレット等は環境変数ではなく `secrets.toml` で自動生成・管理する |
| 外部サービス連携 | `TUNNEL_TOKEN`（Cloudflare Tunnel）、`CLOUDFLARE_API_TOKEN`/`CLOUDFLARE_ZONE_ID`（ATPハンドル検証のDNS TXT自動作成。未設定時はHTTP `.well-known` 方式のみにフォールバック）、`ATP_RELAY_URL`（Relayへの`requestCrawl`先。カンマ区切りで複数指定可、既定は`https://bsky.network`）、`PLC_DIRECTORY_BASE_URL`（`did:plc:`の登録・解決先。既定は`https://plc.directory`。E2Eテストではローカルのスタブサーバーに向ける）、`ATP_APPVIEW_URL`（Bsky AppViewのベースURL。既定は`https://public.api.bsky.app`。E2Eテストではローカルのスタブサーバーに向ける） |
| SMTP | 環境変数では設定しない。`site_settings` テーブルで管理し管理者API経由で設定する |
