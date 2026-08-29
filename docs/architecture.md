# アーキテクチャ

「開く」機能の`handlers/open_target.rs`は薄いオーケストレーション層とし、Bskyは`seiran-common::atp`、ActivityPub Actorは`target_resolve`、ActivityPub投稿はインバウンドCreateジョブを再利用する。外部ActivityStreams文書の取得はメディアプロキシと共通のDNS固定・private/loopback拒否・リダイレクト再検証を通す。フロントエンドの`OpenTargetDialog`はQRを同期認識し、重いOCR Workerは読取開始後に動的importする。

対象読者: seiran のコード全体に手を入れる開発者。「今のシステムがどう動いているか」だけを書く。変更の経緯や過去の不具合修正は書かない（必要なら `git log` を見る）。

通報は `ReportModal` と `POST /api/reports` でローカル・Fedi・Bskyを統一し、管理画面の
「通報」タブから台帳閲覧、クローズ、内部コメント、凍結/投稿削除、リモート転送を行う。
SnowflakeのActor IDはブラウザで精度を失わないよう文字列で受け渡す。

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
- `streaming.rs` — `StreamHub`（WebSocket配信。`recipients`方式とMisskey互換チャンネル方式`ChannelKind`/`ChannelScope`の両方を扱う、`docs/protocols.md` 8節）
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

- `docker-compose.yml`（split-role）: `db` / `redis`（ジョブキュー共有に必須）/ `api` / `federation-inbox` / `worker` / `atp-repo` / `frontend` / `nginx`（`docker/nginx.conf`）/ `tunnel`。`config-data` ボリュームで `secrets.toml` を全バックエンド間で共有永続化する。サービス間通信はコンテナ内部DNS（`db:5432`等）を使うため`db`のホスト公開は本来不要だが、運用機へのSSHトンネル経由psqlアクセス（DBeaver等）のため`127.0.0.1:5432`（ループバックのみ）でホストへ公開する（`0.0.0.0`にはしない、#220）。
- `docker-compose.mono.yml`（単一コンテナ）: `db` / `seiran-server`（role=all）/ `frontend` / `nginx`（`docker/nginx.mono.conf`）/ `docker-gen`（`--scale seiran-server=N` によるスケールアウト時に nginx へ反映）/ `tunnel`。Redis サービス自体が存在しない（同一プロセス内で完結するため不要）。`scripts/dev-up.sh`は`db`だけをこのcomposeで起動し、backend（`seiran-server`）はネイティブ`cargo run`でホスト側から接続するため、`db`は同じく`127.0.0.1:5432`（ループバックのみ）でホストへ公開する。
- `db` サービスは両 compose とも `docker/Dockerfile.postgres`（`postgres:16-bookworm` ベースに pg_bigm をソースビルドで組み込み）からビルドする（#97）。`shared_preload_libraries=pg_bigm,pg_stat_statements` を起動コマンドで渡す。`pg_stat_statements`は公式イメージ標準同梱の拡張で、クエリ別の実行時間・呼び出し回数を計測できる（パフォーマンス調査の計測基盤、docs/code_audit_2026-08-05.md P-9）。`shared_preload_libraries`の変更はpostmaster再起動が必要なため、コンテナ再作成前は`CREATE EXTENSION`済みでも`pg_stat_statements`ビューへのクエリは失敗する。
- `GET /health`（`seiran-api`、認証不要）: DBへ`SELECT 1`を発行できるかで200/503を返す外形監視用エンドポイント。「プロセスは起動しているがDBプールが枯渇/切断している」状態を外部監視から検知できるようにする（docs/code_audit_2026-08-05.md R-9）。`Role::All`/`Role::Api`のみで提供され、federation/worker/firehoseロールには無い。
- 自前で管理する2つのDockerイメージ（`docker/Dockerfile`＝seiran-server、`docker/Dockerfile.frontend`＝Vite dev server）はどちらも非rootユーザーで実プロセスを実行する（docs/code_audit_2026-08-05.md S-8）。seiran-serverは`uid:gid=10001:10001`の`seiran`ユーザーで動くが、イメージの`USER`はrootのままにしてある。理由は`config-data`ボリューム（`/app/config`、`secrets.toml`の永続化先）が「新規作成される空ボリューム」だけでなく「以前から中身が入っている既存ボリューム」としてもマウントされうるため（例: 非root化より前にrootで書き込まれた`secrets.toml`が既に入っているボリュームへ、非root化後の新イメージを再デプロイするケース）。Dockerfile側の`chown`はイメージ自身のレイヤーにしか効かず、Dockerが「空ボリュームへイメージの内容ごとコピーする」自動処理も中身が空でない限り発動しないため、既存ボリュームの所有権はrootのまま残り非rootユーザーは読み書きできず起動不能になる（実機の`seiran_config-data`ボリュームで再現・修正確認済み）。そのため`docker/entrypoint.sh`が`ENTRYPOINT`となり、コンテナ起動のたびに（1）rootのまま`/app/config`を`chown -R seiran:seiran`で揃え、（2）`gosu seiran`で非rootへ降格してから`seiran-server`をexecする。postgres公式イメージ等と同じ「起動時だけrootでボリューム所有権を修正し、実プロセスは非rootで動かす」構成。それ以外の書き込みは`/tmp`のみ（`media_probe.rs`のffmpeg一時ファイル）で、`/tmp`はどのユーザーでも書き込み可能なため追加対応不要。frontendは`node:22-alpine`同梱の`node`ユーザー（`uid:gid=1000:1000`）で、こちらはbind mountされるソースを読むだけで永続ボリュームへの書き込みが無いため`USER`を直接固定できる。`db`（`docker/Dockerfile.postgres`）・`redis`・`nginx`・`cloudflared`はいずれもサードパーティの公式/準公式イメージで、同種の権限降格を各イメージが自身のentrypoint内で既に行っているためDockerfileに`USER`を追加していない。

## 4. 認証

ログインとTOTP検証は、暗号鍵をpepperにしたkeyed hashだけを`auth_attempt_log`へ保存し、同一識別子に対する資格情報および同一資格情報に対する識別子を既定10分・5種類までに制限する。ウィンドウ起点は「既定10分前」「直近パスワードリセット完了時刻」「直近ログイン成功時刻（`users.last_login_success_at`）」のうち最も新しい時刻で、ログイン成功のたびに試行種類数カウントがリセットされる。制限拒否が同一IPで既定10分に5回発生すると24時間`auth_ip_blocks`で認証を遮断する。各値、Turnstile鍵、登録IP制限（既定60分・5件）は`site_settings`から管理する。ログイン・登録（メール確認送信・直接登録の両方）はTurnstile設定済みの場合にCloudflare Siteverifyを必須とする。クライアントIPは`ClientIp`（`cf-connecting-ip`/`x-real-ip`/`x-forwarded-for`の順で信頼済みヘッダーからのみ解決、無ければ`None`＝IP系制限は素通り）で解決する。

`user`/`emoji-editor`はDMを除くメンション・返信・引用の宛先を1時間30ユニークユーザーまで、投稿を1時間30通（`moderator`は100通）まで、新規フォローを24時間100人（`moderator`は300人）まで、リスト作成数を5本（`moderator`は30本）まで、リスト最大人数を50人（`moderator`は300人）まで、検索を1時間10回（`moderator`は50回、スクロールによるページング取得は回数に含まない）まで、アップロードを1ファイル10MBまでに制限する。`moderator`のアップロード上限は50MB、`admin`はいずれの制限も対象外でAPI全体の既存アップロード上限100MBのみが適用される。閾値は全て`site_settings`から変更可能（`crates/seiran-api/src/rate_limit.rs`の`role_limit`ヘルパー）。

認証の起点はローカル ID/PW（`seiran-common::auth::local::LocalAuthProvider`）。TOTPを有効化したユーザーはパスワード検証後に5分間有効な用途限定JWTを受け取り、`POST /api/auth/totp/verify`でTOTPまたは使い切りリカバリーコードを検証して初めて通常のJWTを取得する。用途限定JWTは通常JWTとクレーム形状を分け、一般APIの認証には利用できない。管理者はユーザー管理APIでTOTP有効状態とパスキー登録数を確認でき、本人が認証手段を失った場合はTOTP設定を強制解除できる。外部認証プロバイダとの連携や、認証方式を切り替える抽象化レイヤーは存在しない。

TOTPシークレットはAES-256-GCMで暗号化して保存し、リカバリーコードはArgon2ハッシュのみを保存する。認証アプリとリカバリーコードを両方失った場合は、パスワード検証済みの用途限定JWTから登録メールアドレスへ1時間有効な解除リンクを送り、リンクのワンタイムトークン消費時にTOTP設定を削除する。

パスキーはWebAuthn relying party（RP ID=`LOCAL_DOMAIN`、originは既定で`https://{LOCAL_DOMAIN}`、ローカル/E2Eのみ`WEBAUTHN_ORIGIN`で上書き）として実装する。ユーザーは設定画面から複数credentialを名前付きで登録・削除できる。登録はresident key必須・プラットフォーム認証器限定（`webauthn-rs`の`start_google_passkey_in_google_password_manager_only_registration`）で行い、discoverable credentialとして保存する。これによりUSB接続のセキュリティキーは登録できないが、ログイン画面ではメールアドレス/ユーザー名の入力なしに「パスキーを使う」から`start_discoverable_authentication`／`identify_discoverable_authentication`によるusernamelessログインができる。登録・認証チャレンジの状態は`passkey_challenges`へ保存し、5分で失効、完了APIで原子的に削除する（`user_id`は認証開始時点でユーザーが未確定なためNULL許容）。認証成功時は署名カウンター等を含むcredentialを更新して通常JWTを発行する。パスキー自体がフィッシング耐性を持つ強い認証方式のため、パスキーログインではパスワードおよびTOTP入力を要求しない。

- パスワード: Argon2（`argon2` クレート既定パラメータ、`OsRng` で salt生成）
- トークン: `jsonwebtoken` による JWT（HS256相当）。`sub` は `"local|{user_id}"`。自社ログイン発行分（`generate_token`）は有効期限7日、MiAuth発行分（`generate_app_token`）は `exp` クレーム自体を持たず無期限（失効は `app_tokens.revoked_at` のみで管理、後述）。secret は `secrets.toml` の `jwt_secret`（256bit hex、起動時自動生成）。クレームに `iat`（発行時刻）を含み、`users.token_valid_after` より前に発行されたトークンは `extract_auth` で拒否する。パスワード変更（`change-password`・パスワードリセット）時に `token_valid_after` を現在時刻へ更新することで、攻撃者が窃取した旧トークンをパスワード変更だけで一括失効させられる。`iat` を持たない旧トークン（この仕組み導入前に発行）は `token_valid_after` が未設定なら従来どおり有効期限まで有効（デプロイ時の強制全ログアウトを避けるための移行措置）。
  `extract_auth` は検証時に `exp` を無視してデコードし（`verify_token_ignoring_exp`）、`jti` が `app_tokens` に登録された有効な管理対象トークンであれば `exp` の値に関わらず通す。登録が無い（＝自社ログイン等の管理対象外）トークンのみ `exp` を厳密にチェックする。これにより、無期限化の仕組み導入前に発行済みで `exp`（7日）が埋め込まれたままの MiAuth トークンも、無効化されていなければ引き続き有効に扱われる。

**MiAuth 互換**（Misskeyクライアント向け）: `GET /miauth/:session_id`（認可ページ）→ `POST /api/miauth/:session_id/authorize`（要Bearer、認可するとそのユーザーの無期限JWTを発行、`generate_app_token`）→ `POST /api/miauth/:session_id/check`（クライアントがポーリングして受け取る）。セッション状態は `AppState.miauth_sessions`（プロセス内メモリ、DB永続化なし）。発行したトークンは `app_tokens` に記録され（#60）、設定画面から明示的に無効化するまで有効（自社ログインの7日失効は適用されない）。Misskey互換クライアント（Aria等）は「連携したら明示的に取り消すまで有効」という前提で作られているため、無期限としている。

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
| `BskyVideoPoll{media_file_id}` | Bsky公式動画パイプラインの完了ポーリング。起動時リカバリ対象（下記） | — |
| `ProxyFollowSync` | list-relay仮想アクターの代理フォロー同期 | — |
| `AccountWithdrawUnfollowAll{actor_id, username}` | 退会時の一括アンフォロー。起動時リカバリ対象（下記） | — |
| `BskyPostCommitDeferred{actor_id, post_id, pending_media_file_id}` | 動画添付投稿のATPコミットを動画結合完了まで遅延。起動時リカバリ対象（下記） | — |
| `BskyDmSend{post_id}` | DM宛先のBskyアクターへ`chat.bsky.convo.sendMessage`で送信（`docs/protocols.md` 9節） | 高 |
| `RemoteFollowListSync{actor_id, direction}` | リモートFediアクターのfollowers/following全件取得（プロフィール表示時の短タイムアウト同期取得が失敗/タイムアウトした場合のフォールバック、`docs/protocols.md` 2節） | 低 |
| `RemoteActorResolve{uri}` | リモートfollowers/following一覧中、ローカルDB未登録のactor URIのプロフィールを解決し`actors`へupsert（フォロー関係は作らない、`docs/protocols.md` 2節） | 低 |
| `RemoteInstanceInfoResolve{domain}` | リモートインスタンスのnodeinfoを取得し`remote_instance_meta`へキャッシュ（NoteCardリモートサーバー表示、`docs/database.md`参照）。notes API/Misskey互換APIが未キャッシュのドメインを見つけた際に積む | 低 |
| `AlsoKnownAsVerify{owner_actor_id, target_actor_id}` | プロフィールの「別のアカウント」（alsoKnownAs、`docs/protocols.md` 2節）の相互検証結果を`actor_also_known_as`テーブルへキャッシュ更新する。「表示時再検証」パターン（下記）の実例 | 低 |
| `RemoteAlsoKnownAsSync{owner_actor_id}` | リモートFediアクター自身のAP actor文書が公開する`alsoKnownAs`を`actor_also_known_as`へ同期し、取り込んだ各エントリに`AlsoKnownAsVerify`を積む（`docs/protocols.md` 2節） | 低 |
| `FollowImportProcess{request_id}` | フォローインポート（設定画面「🚚 インポート・エクスポート」から改行区切りのID一覧を貼り付け or .txtドラッグ&ドロップで一括フォロー。隠し仕様として各行をカンマ区切りで分割し1列目のみを識別子として読む、Misskeyのフォローエクスポート形式`id,withRepliesフラグ`対応）。`follow_import_items`の`pending`を1件処理し、対象が尽きるか`follow_import_requests.status`が`running`でなくなる（完了/キャンセル）まで自分自身を再度積む「自己再enqueue型」ジョブ（下記） | 低 |

**「自己再enqueue型」ジョブ**: 大量の対象（フォローインポートなら数千件のID）を1件ずつ非同期処理し、進捗を都度DBへ反映しつつキャンセル可能にしたい場合のパターン。対象一覧をDBテーブル（`pending`/`succeeded`/`failed`等のステータス列）に永続化し、ジョブは「未処理を1件取得して処理→成功失敗を問わず自分自身を再度enqueue」を繰り返す。対象が尽きた、またはリクエストの状態が`running`でなくなっていれば再enqueueせずチェーンを終了する。既存の「表示時再検証」パターンと異なり外部トリガー（表示）不要で自走する点、対象を使い切ったら自然に止まる点が特徴。`FollowImportProcess`が最初の実例。レート制限等で一時的に処理を継続できない場合は、`Err`を返してWorkerEngineの指数バックオフに委ねるのではなく、`JobQueue::enqueue_retry`を直接呼んで一定時間後に自分自身を再投入する（`retry_config_for`の指数バックオフはDB接続エラー等の真の失敗時のみ使う設計。`FollowImportProcess`はレート制限超過時に5分間隔でポーリングする）。

フォロー作成の実処理（ATPコミット・`follows` INSERT・AP Follow送信・通知等）は元々 `handlers::follows::create_follow` 内の `&AppState` 依存関数だったが、`FollowImportProcess` ジョブ（`JobContext`から実行される）からも同じ処理を呼ぶ必要が生じたため、`seiran-common::follow_exec::execute_follow` へ `AppState` 非依存の共通関数として切り出した（`FollowExecConfig`が必要なリポジトリ・`AtpCommitService`・`StreamHub`等を束ねる。APIハンドラは`AppState::follow_exec_config()`で組み立て、`JobContext::follow_exec`はWorker起動時に同内容を注入する）。ターゲット文字列（ローカルユーザー名/`user@domain`/`https://...`/`did:...`/ATPハンドル）の種別判定ロジックも`seiran-common::follow_target::classify_follow_target`へ同様に統合されており、`create_follow`・`resolve_and_upsert_target`（リスト機能のメンバー追加）・`FollowImportProcess`の3箇所から共有される。`execute_follow`は呼び出し前から既に対象をフォロー済みだった場合も（`follows`への新規INSERTが発生しないだけで）エラーにせず成功として返すため、`FollowOutcome`に`already_following`フラグを持たせて区別できるようにしている（`FollowRepository::upsert_pending`/`insert_accepted_bsky`の戻り値が新規挿入か既存更新かを返す設計を利用）。フォローインポートの進捗表示はこれを見て「成功」と「既存」を別枠に集計する（`follow_import_item_status`の`already_following`、`docs/database.md`参照）。

**起動時リカバリと`advisory_lock`共通ヘルパー**: `Job::FollowImportProcess`の遅延リトライ（レート制限超過時の`enqueue_retry`）に限らず、`InMemoryJobQueue`のリトライ待ち状態はプロセス内メモリのみで管理され、プロセス再起動で消失する。DB上に「未完了」を示す永続状態を持つ自己完結ジョブがこの影響を受けると、リトライ待ち中にプロセスが再起動しただけで処理が誰にも気づかれず永久に止まってしまう。対策として、`seiran-api::spawn_startup_tasks`が起動のたびに「未完了」を示すDB状態を検出して無条件で全件再enqueueする。現在4つのジョブがこのパターンを採用している:

| ジョブ | 未完了の検出条件 | 起動時リカバリ関数 |
|---|---|---|
| `FollowImportProcess{request_id}` | `follow_import_requests.status='running'` | `resume_running_follow_imports` |
| `AccountWithdrawUnfollowAll{actor_id, username}` | `actors.withdrawn_at IS NOT NULL` かつ `follows`に残存行あり | `resume_account_withdraw_unfollow_all` |
| `BskyVideoPoll{media_file_id}` | `media_files.bsky_video_status = 'pending'` | `resume_bsky_video_poll` |
| `BskyPostCommitDeferred{actor_id, post_id, pending_media_file_id}` | `posts.pending_bsky_media_file_id IS NOT NULL AND at_uri IS NULL` | `resume_bsky_post_commit_deferred` |

「最後に進捗があってから一定時間経過したものだけ」のように絞り込むと、絞り込み条件の見積もり次第で本当に停止している処理を再開し損なう投入漏れの方が実害として大きいため、いずれもあえて絞らない。

この無条件再enqueueは、正常に動いている既存の処理に対しても重複してジョブを積みうる（split-role構成で複数APIレプリカが同時に起動する場合や、Redisキューでプロセス再起動をまたいでジョブが残っていた場合も同様）。特に`FollowImportProcess`は複数チェーンが並行すると`check_follow_rate_limit`のTOCTOU（チェックと実際のフォロー成立の間に他のトランザクションを排除できない）で上限をわずかに超過しうる。これを防ぐため、上記4ジョブはいずれも処理開始時に`crate::advisory_lock::try_acquire(pool, key)`（`FollowImportProcess`は`request_id`、`AccountWithdrawUnfollowAll`は`actor_id`、`BskyVideoPoll`/`BskyPostCommitDeferred`は`media_file_id`/`post_id`をキーに使う）で`pg_try_advisory_lock`を取得できた場合だけ実処理を行い、取れなければ（既に別のジョブが処理中とみなし）何もせず終了する（re-enqueueもしない。動いている方のジョブが自分で処理を継続するため）。異なるジョブ種別のキー空間はそれぞれ別のsnowflake ID採番元（`follow_import_requests.id`/`actors.id`/`media_files.id`/`posts.id`）であり、advisory lockの単一引数版は名前空間を分けていないが、64bit空間での偶然の衝突確率は無視できるため許容している。

advisory lockはセッションスコープのため、`PgPool`から都度借りる接続では`lock`と`unlock`が別コネクションになりうることに注意し、`try_acquire`は`pool.acquire()`で明示的に確保した1本の接続を返し、呼び出し側はそれをそのまま`release`に渡す。**`FollowImportProcess`のように次のジョブをenqueueする設計のものは、そのenqueueを必ずunlock完了後に行う**（unlock前にenqueueすると、別ワーカーが即dequeueして`pg_try_advisory_lock`を試み、まだロックが残っていて失敗し、re-enqueueもされずチェーンが途切れてしまうため）。

**`BskyPostCommitDeferred`のペイロード最小化**: このジョブは元々`text`（本文）・`reply_root`/`reply_parent`（リプライ先at_uri/at_cid）・`now`（投稿作成時刻）をジョブのペイロードとして直接保持していたが、これらはいずれも`posts`テーブルに既に永続化されている情報の写しであり、`InMemoryJobQueue`が消えるとこの写しごと失われ、起動時リカバリで`post_id`だけから元のジョブを再現できなかった。そこで`actor_id`/`post_id`/`pending_media_file_id`のみを持つ設計に変更し、ハンドラが`post_id`から`posts.body`/`created_at`/`reply_to_post_id`を都度取得する（リプライ先at_uri/at_cidは`reply_to_post_id`が指す投稿の`at_uri`/`at_cid`から再構築し、root/parentは常に同じ値を使う。`handlers::notes::delivery::resolve_reply_context`と同じ規約）。`pending_media_file_id`自体は`resolve_bsky_embed`（複数添付間の優先順位判定を含む）の結果を起動時リカバリで再現不要にするため、投稿作成時点で`posts.pending_bsky_media_file_id`へ永続化しておき（`enqueue_bsky_post_commit_deferred`）、コミット成功後にNULLへ戻す。

なお、この排他ロックは同一`request_id`（同一インポート）内の重複実行のみを防ぐものであり、フォローインポート実行中に通常の`POST /api/follows/create`（手動フォロー）が同時に行われた場合の`check_follow_rate_limit`のTOCTOUまでは解消しない。この経路でのレート制限超過は実害が小さいと判断し、あえて対応範囲に含めていない。

**「表示時再検証」パターン**: 外部（他インスタンス等）の状態に依存する値をリアルタイムで検証すると表示のたびに外部フェッチが走り遅延・相手サーバーへの負荷が生じる。かといって一度きりの検証では相手側の状態変化に追随できない。そこで、表示は常にDBキャッシュ済みの値を即座に返しつつ、表示のたびに低優先度の再検証ジョブを積んで非同期でキャッシュを更新する（「今見ている値は少し古いかもしれないが、リロードすればその頃には最新化されている」という体験を許容する設計）。`AlsoKnownAsVerify`が最初の実例で、同様の「外部状態のキャッシュ+閲覧トリガーの非同期再検証」が必要になった箇所では踏襲する想定。

このパターンを採用するジョブが、1回の実行で他の低優先度ジョブを大量にファンアウトする重い処理（例: `RemoteFollowListSync`が未知アクターごとに`RemoteActorResolve`を積む）の場合、表示のたびに無条件でenqueueすると、同じ内容の重いジョブが何度もリロードされるだけで積み重なり、同一優先度を共有する他のジョブを飢餓状態にしうる（#229）。この種のジョブは`AppState`側にプロセス内メモリのクールダウン（`(キー) → 直近enqueue時刻`の`DashMap`、一定時間内の再投入を無視する）を設け、根本的な重複投入を抑える。`enqueue_remote_follow_list_sync`（`remote_follow_sync_recent`、10分）が実例。

**並列・排他制御**: グローバル同時実行数上限（`Semaphore`、既定32、ジョブ単位）、ドメイン単位の同時接続数制限（最大2並列、`RemoteActorResolve`/`RemoteFollowListSync`/`ActorHistorySync`などリモートから取得する系のジョブ用。`JobContext::get_domain_semaphore`）、アクターID単位の直列化（ATPコミットの順序保証）、指数バックオフ+ジッターでのリトライ。AP配送（`ApDelivery`）のinboxファンアウト自体は`fan_out_activity`内で`buffer_unordered`により最大8並列（`crates/seiran-common/src/ap/deliver/infra.rs`）。追加の`tokio::spawn`は行わず、Workerジョブ実行のタスク内でポーリングを並列化するのみ（docs/code_audit_2026-08-05.md P-3）。

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

検索文字列はBluesky AppViewへはそのまま渡し、ローカルDB向けには共通の検索式ASTへ変換する。空白/`+`はAND、`OR`はOR、先頭`-`はNOTとして扱い、引用句と括弧を使用できる。括弧が不足していても入力端に不足分があるものとして解釈する。`from:`・`mentions:`・`domain:`・`since:`・`until:`をローカル投稿にも適用し、`from:me`/`mentions:me`は認証中のローカルactorへ解決する。ローカル/Fedi投稿には言語宣言が保証されないため`lang:`は認識するが常にTRUEとする。SQLはASTからプレースホルダー付きで生成し、LIKEメタ文字をエスケープする。
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

**動画のfaststart化**: `mime_type` が `video/mp4`/`video/quicktime` の添付は、S3保存直前に `media_probe.rs::faststart_video()`（`ffmpeg -c copy -movflags +faststart`、再エンコードなしのコンテナ再mux）を通す。アップロード元ファイルの `moov` アトム（再生に必須のメタデータ・シークテーブル）が `mdat` の後（ファイル末尾）にある「非faststart」なmp4は、ブラウザの `<video>` によるプログレッシブ再生が `moov` を読み込むまで開始できず、ファイルが大きいほど再生開始の遅延・失敗が顕著になる（特にSafari/iOSはほぼ再生不能）。sha256による重複排除判定・ffprobeでのメタデータ抽出・Bsky動画パイプラインへの提出は、いずれもfaststart化前の生バイト列のまま扱う（Bsky側は独自にトランスコードするためfaststart化は不要、重複排除はアップロード元バイト列の同一性で判定するため）。失敗時は元のバイト列をそのまま保存する。

**画像アップロードパイプライン**（`storage/image.rs::prepare_image()`）: ユーザーの画像を不要に劣化させないため、2つの候補を用意してから採用する。まず `storage/exif.rs`（`img-parts`クレート使用）でJPEG/PNGのExifをOrientationタグのみに絞り込んだ「無劣化オリジナル候補」を作る（画素は再エンコードしない）。続けてOrientationを画素に適用したうえで `MediaKind` ごとの最大サイズにリサイズしWebPロスレスエンコードした「リサイズ候補」を作る。呼び出し元（`handlers/media_store.rs::store_image()`）が両候補それぞれのsha256+blurhashで `media_files` の重複排除チェックを行い、どちらも未登録ならバイトサイズが小さい方を採用してS3へアップロードする。img-parts非対応フォーマット（静止画WebP・AVIF・単一フレームGIF等）はOrientation適用のみ行いWebP再エンコードする（オリジナル候補なし）。アニメーション画像（GIF/APNG/WebPアニメ）は元バイト列をそのまま保存する。

**リモートメディアプロキシ（#87）**: フロントエンドは別オリジンのアバター、添付、サムネイル、本文・リアクションのカスタム絵文字を `GET /proxy?url=...` に変換する。同一オリジンのストレージURLは直接参照する。自インスタンスのストレージ（R2等）は`window.location.origin`とは別サブドメインで運用されることが多いため、`/api/meta`が返す有効なstorage providerの公開URL一覧（`internalMediaOrigins`）もフロント（`utils/mediaProxy.ts::configureInternalMediaOrigins()`）に同一オリジン相当として登録し、SSRF対策・容量上限（25MiB）付きのプロキシを経由させず直接参照する（自分のインフラのためCORS/SSRFリスクがなく、動画等25MiBを超えるファイルもプロキシの容量上限に引っかからないようにするため）。内蔵プロキシはHTTP(S)のみを許可し、資格情報・fragmentを拒否、DNS解決した全IPについてloopback/private/link-local/CGNAT等を拒否する。リダイレクト先も都度同じ検証を行い、5回・25MiB・20秒を上限とし、画像・動画・音声以外は中継しない。上流の`Content-Type`がホワイトリストに一致しない場合（`application/octet-stream`を返す配信元がある）、アップロード機能と同じマジックバイト判定（`sniff_mime_type`、`infer`クレート）で実体を判定し直し、それでも一致しなければ中継を拒否する。`site_settings.media_proxy_url` が設定されている場合は、Misskey互換の外部プロキシ `{base}/proxy?url=...` を利用する。SSRF対策を含むこの検証・取得ロジックは `handlers/media_proxy.rs::fetch_validated()` として切り出されており、`/proxy` エンドポイント自体と、リモート絵文字インポート（`handlers/admin/remote_emojis.rs`、#73。取得後は `prepare_image` → `media_store::store_image` を通して通常のアップロードと同じ経路で `media_files`/`custom_emojis` に登録する）の両方から使う。

**未設定アバター（#211）**: ローカル actor にアップロード済みアバターがない場合は、`actor_id` をシードに色相・口・目の配置を決めた SVG を API ロールの `GET /api/avatars/:actor_id` で返す。同じ ID の画像は不変なため `immutable` で長期キャッシュする。生成ロジックと URL 組み立ては `seiran-common::avatar` に集約する。公開 URL を `/api` 配下に置くことで Cloudflare Tunnel の既存 backend ルーティングを利用する。

**リモート絵文字カタログ・インポート（#73）**: AP受信（投稿本文・表示名・絵文字リアクションのいずれか）で見つけたカスタム絵文字は `remote_emojis` テーブルへ都度 `upsert_seen` される（画像自体は取り込まない、カタログのみ）。管理画面「絵文字」パネルの「リモート」タブと、NoteCard本文・絵文字リアクションの右クリックメニュー（管理者にのみ表示、`components/note/EmojiContextMenu.tsx`）の双方が、この一覧からの1件選択→カテゴリ/タグ/ライセンス入力ダイアログ（`components/admin/EmojiImportDialog.tsx`）→`POST /api/admin/emojis/remote/import` という同じ導線でローカルの `custom_emojis` へ取り込む。

未設定アバターの顔は、目の間隔を 18/23/28 の3段階、口を各原型の80%サイズとする。笑顔の一種は上辺が直線で下側が曲線のD型とし、上端をほかの口と同程度の高さに揃える。フロントエンド用API・Misskey互換APIとも、ローカルアクターの画像が未設定なら同じ代替URLを返す。生成仕様を変更した際は、immutableキャッシュを更新できるよう代替アバターURLの `v` クエリも更新する。

代替アバターの配信形式は、SVG非対応のMisskeyクライアントでも表示できるようPNGとする。APIは `image/png` を返し、形式変更時のimmutableキャッシュを避けるためURL版数を `v=5` とする。`ATP_BACKFILL_UNSET_AVATAR_PROFILES_ONCE=1` でAPIロールを一度だけ起動すると、画像未設定の全ローカルactorについて現在のATPプロフィールを再コミットし、Relay/AppViewへ再取得を促せる。通常起動では実行しない。

## 8. フロントエンド

React 18 + Vite + TypeScript（react-router-dom v7、declarative mode。`<BrowserRouter>`＋`useNavigate`/`useParams`等のフック中心で、データルーター（`createBrowserRouter`等）は不使用）。`frontend/src/` 構成:

- `api/client.ts` — バックエンドAPIクライアント、`ApiError`、`getErrorMessage()`
- `components/layout/` — `AppShell`（3ペインの外枠）、`LeftNav`
- `components/note/` — `NoteCard`（タイムライン・詳細・プロフィール共通の投稿カード）、`PostComposer`、`ReactionChips`（各チップのホバーでリアクター一覧をポップオーバー表示、`ReplyIndicator`と同じ遅延フェッチ・遅延クローズパターン）/`ReactionPicker`（トリガーボタン＋`Modal`内の`EmojiPickerPanel`。Unicode絵文字データセット（`unicode-emoji-json`）は`React.lazy`で遅延ロードし、カスタム絵文字とあわせて検索・タブ切り替えで選べる。CLDRアノテーション検索索引（`lib/emojiAnnotations.ts`）は`emojibase-data`の生JSON（1言語700〜800kB、hexcode/group/skins等未使用フィールド込み）を直接importせず、`scripts/build-emoji-annotations.mjs`がpostinstallで生成するemoji/label/tagsだけの軽量JSON（`src/generated/emoji-annotations/`、git管理外、1言語170〜220kB）を言語ごとに動的importする。docs/code_audit_2026-08-05.md P-7）、`HlsVideo`、`RichText`（本文中のMarkdownリンク`[text](url)`・生URL・`@mention`・`#ハッシュタグ`・絵文字ショートコードを1パスでクリック可能な要素へ変換。AP由来のハッシュタグアンカー`[#foo](リモートURL)`もリンクテキストの形状で検出し自インスタンスの`/tags/foo`へ読み替える。`EmojiText`は表示名等リンク化不要な箇所向けにショートコード置換のみ残す）等
- `NoteCard` の引用表示（#116）は、APIの `quote` に引用元 `NoteResponse` を1段だけ埋め込み、本文直下の枠付きカードとして描画する。カードは返信マーカー、ユーザー、CW、本文、時刻、添付、アンケート、リアクションを再利用し、引用元自身の `quote_id` は「引用あり」表示だけに留めて再帰描画しない。
- `components/right/` — 右ペインのタブ内容（`NotificationsPanel`、`TrendsSearchPanel`、`FollowListPanel`、`AuthorPanel`（ポスト詳細画面の「投稿者」タブ。投稿主のプロフィール概要と固定ポストを表示、`docs/ui_spec.md` 2.3参照）、`ReplyThreadPanel`（ポスト詳細画面の「返信」タブ。`GET /api/notes/:id/replies`のフラット配列から`replyId`/`quoteId`を辿ってツリーを再構築、`docs/ui_spec.md` 2.3参照）、`ReactionListPanel`（ポスト詳細画面の「リアクション」タブ。絵文字ごとにグループ化しアクター一覧を常時展開表示、`docs/ui_spec.md` 2.3参照）、`RepostListPanel`（ポスト詳細画面の「リポスト」タブ。`GET /api/notes/:id/reposts`で`posts.repost_of_post_id`を辿り、取り消し済みも含めた履歴を表示、`docs/ui_spec.md` 2.3参照））
- `components/admin/` — 管理画面パネル群
- `components/dm/` — `RecipientPicker`（DM宛先のchip入力。サジェスト選択/手打ち確定の両対応、Bskyアクターと他プロトコルの混在を警告表示）
- アクター検索APIは用途別に分離する。`GET /api/actors/search`はリスト編集・DM向けの表示名/全ハンドル部分一致、`GET /api/actors/suggest`は`ComposerEditor`向けのハンドル前方一致である。後者のレスポンス`target`は入力形式に応じてローカル短縮/Fedi/Bsky表記を選び、フロントはその値をそのまま挿入する。
- `contexts/` — `AuthContext`（起動時のセッション確認は`contexts/authSession.ts`の`resolveSession()`が担う。`GET /api/auth/me`が明示的な401（認証失効）を返した場合のみトークンを破棄してログアウトし、それ以外の失敗（バックエンド再起動中の接続断・5xx等）は`AUTH_ME_RETRY_DELAYS_MS`（1s/2s/4s）でリトライし、それでも解決しなければトークンを保持したまま諦める。ログイン状態がバックエンド再起動のたびに失われるのを防ぐ設計）、`ComposerContext`（返信モーダルに加え、`openCompose(initialText)` で本文プリフィル済みの素の投稿モーダルもグローバルに開ける）、`RightPaneContext`（右ペインのサブタブ状態保持。ポスト詳細画面の「前後のポスト」タブのスクロール位置もポストIDごとにインメモリで保持する、`docs/ui_spec.md` 2.4参照）、`StreamingContext`（WebSocket集約。タイムライン新着ノートはMisskey互換のチャンネル購読方式（`subscribeChannel(spec, onNote)`、`docs/protocols.md` 8節）で配信される。`type:"channel"`イベントのうち`body.type==="note"`を、受信後に閲覧者権限付き`GET /api/notes/:id`で完全な`NoteResponse`へ補完し、受信経路ごとの簡易ペイロード差にかかわらず引用・アンケート・添付をリロードなしで表示する（`resolveStreamNote`、補完取得失敗時はストリームペイロードへフォールバック）。購読中チャンネル一覧は`useRef`で保持し、`useStreaming`の`onOpen`コールバック（WebSocket再接続を検知）で`connect`を全件再送する。`HomePage`は表示中タブ（`Feed`）が変わるたびに`subscribeChannel`し直し、旧チャンネルを`disconnect`する。DM新着（`visibility=direct`のnoteイベント、チャンネル購読不要の`recipients`方式のまま）は`registerDirectMessage`で別系統に振り分け、未読セッション数`dmUnreadCount`をLeftNavのバッジに供給する。Fediフォロー承認（`followAccepted`）受信時は`stores/followStatusStore`を直接更新する）、`ToastContext`（エラー/成功/情報トースト通知）、`NavigationHistoryContext`（`useGoBack()`。SPA内でPUSHされたナビゲーションの深さを追跡し、各画面の「戻る」ボタンから共通で使う。直接URLを踏む・リロードする等でSPA内に戻り先が無い場合は`navigate(-1)`の代わりにホーム（`/`）へ遷移する）
- `stores/followStatusStore.ts` — フォロー状態（`not_following`/`pending`/`accepted`）の外部ストア（Reactコンテキストではなくモジュールスコープの`Map`+`useSyncExternalStore`）。キーは`lib/format.ts`の`profileQuery(username, domain)`で統一。同一アクターのフォロー状態表示は画面内に複数存在しうる（プロフィール本体、右ペインのポストリスト、タイムライン上の同一ユーザーの複数投稿）ため、各コンポーネントがローカルstateで抱えず全てこのストアを購読する設計にし、一箇所の操作（フォロー/フォロー解除ボタン）・WebSocket経由の`followAccepted`受信のいずれでも表示中の全コンポーネントへ同時反映する。`ProfilePage`のフォローボタン、`NoteCard`のタイムライン上のフォロースイッチが利用
- `pages/` — 画面単位のトップレベルコンポーネント（`MessagesPage`はDM専用画面、`docs/ui_spec.md`参照）
- `i18n/` — 国際化。`react-i18next` + `i18next-browser-languagedetector`。表示言語（`i18n.displayLanguages`、8言語: `en`/`ja`/`zh-Hant`/`zh-Hans`/`ko`/`es`/`de`/`fr`）とポスト言語（`i18n.postLanguages`、7言語: `en`/`ja`/`zh`/`ko`/`es`/`de`/`fr`）を別リストとして持つ。表示言語のみ中国語が繁體（`zh-Hant`）/简体（`zh-Hans`）のバリエーションを持ち、ポスト言語側は`zh`単一のまま（`seiran_common::SUPPORTED_LANGUAGES`と一致）。`postLanguageBase()`が表示言語からポスト言語のデフォルト値を導出し、`zh-Hant`/`zh-Hans`のどちらでも`zh`に丸める（`PostComposer`のデフォルト言語、絵文字アノテーション読み込み言語の両方で使用）。「自動」時はブラウザの言語設定に従い判定し、`normalizeDetectedLanguage()`が地域コード（`zh-TW`/`zh-HK`/`zh-MO`→`zh-Hant`、それ以外の`zh`系→`zh-Hans`、他言語は言語部分のみ）へ正規化する（対応言語外は `en` にフォールバック）。この正規化は`i18n.init`の`detection.convertDetectedLanguage`とonload設定`load: "currentOnly"`（`zh-Hant`/`zh-Hans`が`languageOnly`設定で`zh`に丸められてしまうのを避けるため）にも使う。設定画面「表示」（`/settings/appearance`、#55・#138）でユーザーが明示的に言語を選択した場合は`localStorage`に記憶（`detection.caches`）し、ログイン中はさらに`users.language_preference`（サーバー保存値）が優先される（`AuthContext`がログイン/`GET /api/auth/me`取得時に`i18n.changeLanguage()`を適用）。翻訳リソースは `i18n/locales/{displayLanguage}/{namespace}.json` に画面・機能単位の名前空間で分割配置し、Viteの`import.meta.glob`で全対応表示言語・名前空間を集約してビルド時にバンドルする。`i18n/index.test.ts`が全表示言語の名前空間・キー・補間変数の一致を保証する。名前空間分割は、将来ユーザーが独自の言語ファイル（同形式のJSON）を作成・適用・配布できるようにする構想を見据えたもので、`i18n.addResourceBundle()` により実行時にリソースを追加・上書きできる

3ペインUIのレイアウト仕様は `docs/ui_spec.md` を参照。

**ローカル開発サーバーの標準運用**: seiranの開発はagentic codingが主体のため、HMR付きdevサーバー（`npm run dev`、5173番、React `<StrictMode>`のeffect二重実行など開発ビルド固有の挙動を持つ）ではなく、`npm run build:watch`（`vite build --watch`、ファイル変更のたびに本番相当のプロダクションビルドへ差分ビルド）と`npm run preview`（`vite preview`、`dist/`を配信する静的サーバー、既定4174番・`PREVIEW_PORT`で上書き可）の2プロセスを常駐させ、常にビルド済み・本番相当のコードで動作確認するのを標準とする。バックエンドの`FRONTEND_ORIGIN`（OGP注入・`/notes`等の転送先）も既定でこのpreviewサーバーを指す。開発ビルド固有の挙動は本番ビルドと異なりうるため、不具合調査は常にビルド済みの状態で行う。人力でUIを細かく調整する際は`FRONTEND_ORIGIN`を一時的に`http://localhost:5173`へ書き換えて`npm run dev`を使ってよい。

**ローカル開発サーバーの健全性確認・復旧**: `ps`でプロセスが生きていることは、正しく機能していることを保証しない。特に`vite build --watch`は、ネイティブのファイル監視（inotify）が本開発環境では信頼できず、**ファイル変更検知だけが静かに止まってもプロセス自体は生き続け、エラーも出さない**ことがある（実機確認: 再起動しても再発した）。この状態だと`dist/`は古いビルドのまま固定され、コードを変更したのに`vite preview`（4174番）で確認しても反映されないという分かりにくい不具合になる。そのため`scripts/dev-up.sh`は`vite build --watch`起動時に常に`CHOKIDAR_USEPOLLING=true`（ポーリング方式でのファイル監視）を付与し、この問題を回避している。それでも疑わしいときは:

- `ls -la frontend/dist/assets/*.js` の更新時刻が直近の編集より新しいか確認する（`vite build --watch`のログに`built in ...ms`が出ているだけでは、それが最新の編集を含むビルドとは限らない。ビルド後の成果物ファイル名にはコンテンツハッシュが含まれるため、編集前後でファイル名が変わっていなければ内容も変わっていない）。
- 更新されていなければ、既存プロセスを`kill`してから`cd frontend && CHOKIDAR_USEPOLLING=true npm run build:watch`を再実行する（`vite preview`側は`dist/`を読み直すだけなので再起動不要）。
- バックエンド（`cargo run -p seiran-server`、3000番）が応答しない場合も同様に、まず`ps`ではなく`curl localhost:3000/`等の実応答で確認し、落ちていれば`cargo run -p seiran-server`を再実行する。
- 上記のような「プロセスは生きているのに動作がおかしい」系の不具合に遭遇したら、`df -h`でディスク容量も疑う。`target/`（Rustビルドキャッシュ）は肥大化しやすく、逼迫するとビルド・プロセスの挙動が原因不明な形で壊れる。空き容量が少なければ`cargo clean`で回収できる。

**開発用プロキシとVite内部パスの衝突**: `frontend/vite.config.ts` の開発サーバー（ローカル `cargo run` 直接起動時のみ有効）は `GET /@:handle`（プロフィールページ）をバックエンドへ転送するが、単純なプレフィックスマッチだとVite自身の内部モジュール（`/@vite/client`・`/@react-refresh`・`/@fs/...`・`/@id/...`）まで巻き込んでバックエンドへ転送してしまい、Viteクライアントが読み込めず白画面になる（実機確認）。そのためこれらを除外する正規表現（`^`始まりはVite側でregex扱い）を使う。

`AuthContext`のグローバル401処理は、任意APIの401で即時ログアウトせず、通知を抑止した`GET /api/auth/me`を上記と同じリトライ方針で再確認する。認証失効が確定した場合だけログアウトし、複数APIの同時401では再確認を一本化する（#108）。

## 8.1 OGP (Open Graph) 対応

フロントエンドは SPA のため、素の index.html には投稿・プロフィールごとの `<meta>` が無い。
User-Agent で既知の bot だけを判定して出し分ける方式は、リストにない未知のクローラーを
取りこぼすため採用していない。代わりに `/notes/:id`・`/@:handle`（AP クライアント向け
`Accept` を除く）へのリクエストは常に、SPA の index.html の `<head>` に OGP `<meta>` を
注入したものを返す。実ブラウザはそのまま SPA が起動し（`<meta>` 注入以外は普段と同じ
index.html）、クローラーは JS を実行しないため `<meta>` だけを読んで終わる。

- `crates/seiran-api/src/handlers/ogp.rs` — DB から投稿/アクターの情報を取得し、
  `state.frontend_origin`（Docker既定は`http://frontend:5173`、ローカルネイティブ開発の
  標準は`vite preview`＝ビルド済み本番相当、環境変数`FRONTEND_ORIGIN`で上書き可）から
  index.html を取得して `<title>`・OGP/Twitter Card の `<meta>` を注入する。`GET /notes/:id`
  は既存の AP Note エンドポイント（`get_note_ap`）が `Accept` ヘッダーで分岐し、AP クライアント
  向け JSON-LD とこの OGP 注入 HTML を出し分ける。`GET /@:handle` はプロフィール専用の
  新規エンドポイント。
- 投稿・アクターが見つからない/DBエラー時は `<meta>` を注入せず index.html をそのまま返す
  （ここで 404 等を返すと SPA 自体が起動できず、フロント側の「見つかりません」表示や
  リモートアクターの都度フェッチが機能しなくなるため）。
- 可視性は投稿・プロフィールいずれも通常の閲覧経路と同じ判定を通す（`followers_only`/
  `direct` は非表示、`PostRepository::find_by_id_for_viewer` を viewer なしで呼ぶ）。
- nginx（`docker/nginx.conf`/`docker/nginx.mono.conf`）・ローカル開発（`frontend/vite.config.ts`
  の proxy）とも、`/notes`・`/@` は bot 判定なしで常に api（バックエンド）へ転送する。
- リポスト（Announce）の AP canonical URL は `/notes/:id` ではなく `/announces/:id`
  （`create_repost`、`ap/deliver/announce.rs`）。フロントエンド上のリポストラッパー個別ページは
  通常ポストと同じ `/notes/:id` で表示するため、リモートユーザーが `/announces/:id` へ
  直接ブラウザでジャンプしてきた場合は `GET /announces/:id`
  （`handlers::notes::get_announce_redirect`）が `/notes/:id` へリダイレクトする
  （AP クライアント向け Accept の場合は Announce オブジェクト応答が未実装のため 404）。
  nginx・Vite proxy とも `/notes` と同様に `/announces` を api（バックエンド）へ転送する
  設定が必要。

## 9. E2Eテスト

- PRおよび`main`へのpushではGitHub Actionsの`E2E` jobが、Node.js 20・Chromium・
  E2E専用PostgreSQLを用いて全Playwrightテストを実行する。失敗時はtrace等の
  `playwright-report` / `test-results`を7日間artifactとして保存する。
  セットアップ短縮のため、rust-cacheを`Rust` jobと`shared-key`で共有（`E2E`側は
  `save-if: false`の読み取り専用）、pg_bigm入りPostgresイメージ（`docker/Dockerfile.postgres`）
  はDocker BuildxのGHAキャッシュでビルド、Playwrightブラウザ本体も`actions/cache`で
  キャッシュする。
- frontendのVitestユニットテストもCIの`Frontend` jobで型チェック・lintと併せて
  必ず実行する。
- Rustの`Rust` jobは`cargo fmt --all -- --check`と警告をエラー扱いするClippyを実行する。
  frontendのlintもESLint警告を許容せず、エラー・警告のいずれでもCIを失敗させる。
- `Rust`/`E2E`両jobの`dtolnay/rust-toolchain@stable`は`toolchain`をバージョン固定している
  （実機確認: 固定していないとRustの新しいstableリリースのたびにClippyへ新規lintが追加され、
  コード変更が無くてもCIが突然落ちることがある）。更新は意図したタイミングでのみ、両jobの
  バージョン文字列を揃えて行う。

`e2e/`ディレクトリにPlaywrightプロジェクトを置く。外部の実サービス（fedi/Bskyインスタンス、PLCディレクトリ、Bsky Relay等）とは通信せず、seiranが話す相手をすべてローカルのスタブ/専用インスタンスに置き換えた上で実行する。実行は `cd e2e && npm test`。

- `e2e/playwright.config.ts`: `webServer`にスタブPLCサーバー・スタブAppViewサーバー・スタブFediサーバー（`stub-fedi-server.ts`、後述）・backend（`cargo run -p seiran-server`）・frontend（`npm run dev`）をまとめて起動する。backendには`PLC_DIRECTORY_BASE_URL`/`ATP_APPVIEW_URL`をそれぞれのスタブサーバーへ、`ATP_RELAY_URL`を存在しないローカルポートへ向け、`CLOUDFLARE_API_TOKEN`/`CLOUDFLARE_ZONE_ID`を空文字にして、外部への実通信を確実に遮断している。`SQLX_OFFLINE=true`も設定し、マイグレーション未適用の空DBに対してsqlxのコンパイル時クエリ検証が失敗しないようコミット済み`.sqlx/`キャッシュを使わせる。
  - 【重要】全`webServer`エントリの`reuseExistingServer`は`false`固定（変更禁止）。backendPort(3000)/frontendPort(5173)は`scripts/dev-up.sh`のネイティブ開発サーバーとも共有しており、`true`だと起動中の実開発サーバーへ無条件に相乗りしてしまう。2026-07-20に実際に発生し、実開発DBへのテストデータ混入・本物のplc.directoryへの誤登録という事故になった（後者は`did:plc:`のtombstoneオペレーションで収束済み）。`false`ならポート競合時に明確なエラーで停止する。
  - 【重要】Playwrightの実行順序は直感に反して「webServer起動 → globalSetup」（`globalSetup`ではwebServerの起動には間に合わない）。そのためE2E専用Postgres（`e2e/docker-compose.yml`、ポート5433）の起動待ちは`globalSetup`ではなく`e2e/scripts/wait-for-db.ts`としてbackendの`command`自体の前段に組み込んでいる。逆に`e2e/global-setup.ts`は「backendが起動済み」を前提にできるので、初期管理者アカウントのbootstrapに使っている（`GET /api/setup/status`は`users`テーブルが1件でもあれば`initialized:true`を返し、未初期化だとフロントは`App.tsx`のルーティングを無視して常に`<Setup>`画面を表示するため、E2E専用DBは空の状態からテストを始める都合上これが必要）。`globalTeardown`はE2E専用Postgresを`down -v`で破棄する。
  - テストは3つのPlaywright projectに分けて並列実行する（`workers: 3`(CI)/`4`(ローカル)）。大半のspecは`main`で並列実行し、`storage_providers`（`is_active`先頭優先のためstub S3登録が競合する）に触れる`notifications`/`misskey-compat`/`federation-delivery`は`storage-serial`（project内`workers: 1`で直列、`main`とはインターリーブ）、`site_settings`のグローバル変更や外部サービススタブのプロセスグローバル状態に触れる`admin`/`rate-limit`/`search`は`globals-serial`（`dependencies`で`main`・`storage-serial`完了後の排他テール）に隔離する。
- `e2e/fixtures/stub-plc-server.ts`: `plc.directory`のスタブ実装。TypeScriptを`tsx`で直接実行するため、CIのNode.js 20を含め事前ビルドは不要。genesis opを受け取ってメモリに保持し、GET時にDIDドキュメント形式へ組み直して返す。
- `e2e/fixtures/stub-appview-server.ts`: Bsky AppView（`public.api.bsky.app`）のスタブ実装。`app.bsky.feed.searchPosts`等の主要エンドポイントに対し常に空の結果を返す（seiranのローカルDB検索はこれと独立して機能するため、ローカル投稿の検索はAppViewが空でも成立する）。
- `e2e/fixtures/stub-fedi-server.ts`: リモートのActivityPubアクター（Mastodon等）のスタブ実装。正規のHTTP Signatures（RSA-SHA256、Digestヘッダー必須。`crates/seiran-common/src/ap/client.rs`のcanonical signing string規約に準拠）で署名したFollowをseiranの`/inbox`へ送り、フォロー成立後の投稿・返信・リポスト配送（Fedi配送はローカルアクターのacceptedフォロワー全員へのファンアウトのみで、返信先個人への直接配送やsharedInboxは無い。`crates/seiran-common/src/ap/deliver/`）を自身のinboxで受信・記録できる。
- `e2e/fixtures/api-helpers.ts`: テスト対象でないセットアップ（フォロー相手ユーザーの作成等）はUI操作ではなく`/api/auth/register`を直接叩いて済ませ、各テストは検証したいUI操作に集中させる。ログイン状態が前提のテストは`seedAuth()`でlocalStorageにtokenを仕込みUIログイン操作自体を省略できる（ログインフロー自体を検証する`login.spec.ts`だけは実際にフォームを操作する）。
- テストファイルは`e2e/tests/`配下（`signup`・`login`・`post`・`follow`・`reply`・`reaction`・`search`・`profile-edit`・`hashtag`・`federation-delivery`）。DBはテスト実行全体で共有されるため、各テストはユーザー名が衝突しないよう一意なプレフィックス+タイムスタンプで登録する。
- フロントは`i18next-browser-languagedetector`がブラウザロケールを見て言語を決めるため、Playwright側は`use.locale`を`ja-JP`に固定している（既定の`en-US`だとUIが英語化される）。
- DBはE2E専用インスタンスを使い、テスト実行のたびに空の状態から始める（アカウントは各テストが必要に応じて新規作成する）。手動検証用の`seiran{n}`アカウント（本ファイル冒頭のCLAUDE.md参照）とは分離されている。
- Cloudflare DNS（ATPハンドル検証のTXT自動登録）、通知UI（未実装）はE2Eのスコープ外。

## 10. 環境変数

| カテゴリ | 変数 |
|---|---|
| ドメイン | 自ホストドメインは`instance_domain`テーブル（一度確定したら不変、`seiran_common::LocalDomain`/`repository::InstanceDomainRepository`）から起動時に一度だけ読み込む。未確定の場合のみ`LOCAL_DOMAIN`環境変数の値をそのままDBへ書き込んで確定させる後方互換パスがある（確定済み環境では無視される）。`.env`にも`LOCAL_DOMAIN`もDB確定値も無い新規インストールでは、初回セットアップ（`POST /api/setup`）時にリクエストの`Host`ヘッダー（`X-Forwarded-Host`等は見ない生の`Host`のみ、`handlers::setup::host_domain_candidate`）から自動確定する。`GET /api/setup/status`が事前にHostヘッダー由来の候補を`domain_candidate`として返し、フロントが確認表示後にそのまま`POST /api/setup`のリクエストボディへ送り返す。サーバー側は送信時点の実際の`Host`ヘッダーとリクエストボディの値が完全一致することを検証してから確定する（`handlers::setup::try_confirm_domain`、不一致は`DOMAIN_MISMATCH`で拒否）。Hostヘッダーが無い・`localhost`・IPアドレス直打ちの場合は「シングルホストモード」（連合なし、PLC genesisを行わずAT Protocol DIDを持たないローカルユーザーとして開始、`actors.domain='localhost'`）で起動する。`ATP_PDS_ORIGIN`は廃止済み（未使用だったため） |
| 起動ポート | `PORT`(既定3000), `FEDERATION_INBOX_PORT`(既定3001) |
| データベース | `POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB`、`DB_HOST`/`DB_PORT`（既定`localhost`/5432。Docker運用では`docker-compose.yml`が`DB_HOST=db`を注入）、`DB_MAX_CONNECTIONS`（プール最大接続数、既定10。split-roleではプロセスごとに持つ）。接続先はこれらから組み立てる（`DATABASE_URL`という完成済みURL変数は持たない、`seiran_common::db::get_db_pool`） |
| ジョブキュー | `REDIS_URL`（split-role構成専用。`--role all` では不要） |
| シークレット | `SEIRAN_CONFIG_DIR`（既定 `./config`）。JWTシークレット等は環境変数ではなく `secrets.toml` で自動生成・管理する |
| 外部サービス連携 | `TUNNEL_TOKEN`（Cloudflare Tunnel）、`CLOUDFLARE_API_TOKEN`/`CLOUDFLARE_ZONE_ID`（ATPハンドル検証のDNS TXT自動作成。未設定時はHTTP `.well-known` 方式のみにフォールバック）、`ATP_RELAY_URL`（Relayへの`requestCrawl`先。カンマ区切りで複数指定可、既定は`https://bsky.network`）、`PLC_DIRECTORY_BASE_URL`（`did:plc:`の登録・解決先。既定は`https://plc.directory`。E2Eテストではローカルのスタブサーバーに向ける）、`ATP_APPVIEW_URL`（Bsky AppViewのベースURL。既定は`https://api.bsky.app`。E2Eテストではローカルのスタブサーバーに向ける） |
| SMTP | 環境変数では設定しない。`site_settings` テーブルで管理し管理者API経由で設定する |
