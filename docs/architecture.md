# アーキテクチャ

対象読者: seiran のコード全体に手を入れる開発者。現在の動作だけを書く（経緯は `git log`）。

## 1. プロトコル上の位置づけ

seiran は Fediverse (ActivityPub) と Bluesky (AT Protocol) の両方にサーバーとして参加する。

- **AP側**: 一般的な Fedi インスタンスと同じく Actor・Inbox・Outbox・WebFinger を持つ。
- **ATP側**: 外部 PDS を使わず、seiran 自身が各ローカルユーザーの PDS を兼ねる。ユーザーごとに `did:plc` を発行し、投稿のたびに MST をコミット・P-256 署名して公式 Relay（`bsky.network`）へ配信する。AppView は Bluesky 公式のものを使う。

AP は普通のサーバー実装だが、ATP は PDS を自作している。この非対称性が実装の複雑さの主な発生源。

## 2. ワークスペース構成

workspace は6 crate。実行バイナリは `seiran-server` だけで、他は lib。

| crate | 役割 |
|---|---|
| `seiran-common` | 共通基盤。DB・認証・シークレット・ジョブキューとジョブ実処理・AP/ATP クライアント・Repository 層・ストレージ・ストリーミング |
| `seiran-api` | Web API。カスタム API・Misskey 互換 API・MiAuth・XRPC・drive・admin。axum Router と `AppState` |
| `seiran-federation-inbox` | AP の受信と公開エンドポイント（inbox・webfinger・actor・outbox・nodeinfo・featured/lists） |
| `seiran-federation-worker` | ワーカーエンジンの起動だけ。ジョブの実処理は `seiran-common::jobs` |
| `seiran-atp-repo` | Jetstream 購読（投稿・Like・Repost）、Bsky DM・フォロワー・ブロックのポーリング/監視 |
| `seiran-server` | `--role` に応じて上記を配線して起動する |

`seiran-common` の主なモジュール:
- `auth/local.rs` — ローカル認証（Argon2 + JWT）
- `secrets.rs` — `secrets.toml` の自動生成
- `queue/` — `JobQueue` の InMemory/Redis 実装とワーカーエンジン
- `jobs/` — 各ジョブの実処理
- `ap/` — AP クライアント・配送・WebFinger・outbox
- `atp/` — MST/リポジトリ、PLC、DID 解決、サービス間認証
- `repository/` — Repository 層
- `storage/` — S3 互換クライアント、ストレージ選択、画像処理
- `streaming.rs` — `StreamHub`（`recipients` 方式とチャンネル方式、`docs/protocols.md` 8節）
- `net.rs` — SSRF 対策込みの HTTP 取得・連合用クライアント
- `id.rs` — Snowflake ID
- `jetstream_control.rs` / `jetstream_leader.rs` — Jetstream 接続のプロセス間調整

## 2.1 バージョン管理と互換性チェック

frontend と backend は1つのシステムバージョンを共有する。`Cargo.toml` の `[workspace.package].version` と `frontend/package.json` の `version` を同じ値にし、各 crate は `version.workspace = true` で参照する。フロントは `vite.config.ts` の `define` で `__FRONTEND_VERSION__` として埋め込み、`src/version.ts` の `FRONTEND_VERSION` で参照する。

`version` とは別の行に任意のサフィックスを持てる:
- Rust: `[workspace.package].version_suffix`（既定は空）。Cargo の既知フィールドではないので `unused manifest key` 警告が出るが無害。`crates/seiran-common/build.rs` がルートの `Cargo.toml` を直接パースして `SEIRAN_VERSION_SUFFIX` として渡し、`version.rs` の `SERVER_VERSION` が `concat!(env!("CARGO_PKG_VERSION"), env!("SEIRAN_VERSION_SUFFIX"))` で結合する。`seiran-api` に依存しない crate（nodeinfo 等）からも参照するので `seiran-common` に置く。
- フロント: `package.json` の `versionSuffix`。`vite.config.ts` が `version` と結合する。

別行にしているのは、ブランチやフォークでサフィックスだけを変えてもコンフリクトしないようにするため（main で `-dev`、リリースブランチで空、フォークで `-some-fork` 等）。

両者は「対応する対向の最低バージョン」も持つ:
- サーバー: `version.rs` の `SERVER_MIN_PEER_VERSION`（要求するフロントの最低バージョン）
- フロント: `version.ts` の `FRONTEND_MIN_PEER_VERSION`（要求するサーバーの最低バージョン）

サーバーは `middleware::version_headers::attach` で全レスポンスに `x-seiran-server-version`/`x-seiran-server-min-peer-version` を付ける。nodeinfo の `software.version` も `SERVER_VERSION`。

フロントは `api/core.ts` の `request()`/`uploadFormData()` が全レスポンスで `api/versionCompat.ts::checkVersionCompat()` を呼び、「フロントのバージョン ≥ サーバーの最低対向バージョン」と「サーバーのバージョン ≥ フロントの最低対向バージョン」のどちらかが崩れたら `ReloadRequiredDialog` でリロードを促す。閉じると同じセッション中は再表示しない。

LeftNav 左下の「Powered by Seiran」から開く `ServerInfoDialog` は、フロントのバージョンと `SiteMetaContext` の `serverVersion` を表示する。`serverVersion` は起動時の `GET /api/meta` を初期値に、以後はレスポンスヘッダーを `setServerVersionHandler()` 経由で受けて更新するので、デプロイし直すと次の API 呼び出しで表示が変わる。

## 3. 統合バイナリとロール分割

`seiran-server/src/main.rs` の `Role::resolve()` が `--role=xxx` → `SEIRAN_ROLE` → 既定 `All` の順で決める。

| CLI値 | Role | 対応crate | ポート |
|---|---|---|---|
| `all`（既定） | All | 全部 | `PORT`（既定3000） |
| `api` | Api | seiran-api | `PORT` |
| `federation` / `inbox` | Federation | seiran-federation-inbox | `FEDERATION_INBOX_PORT`（既定3001） |
| `worker` | Worker | seiran-federation-worker | なし |
| `firehose` / `atp-repo` | Firehose | seiran-atp-repo | なし |

- **All**（`run_all`）: DB・シークレット・HTTP クライアント・`job_queue`（常に InMemory）を1回だけ作り、`seiran_api::router().merge(seiran_federation_inbox::router())` を1ポートで待ち受ける。firehose と worker は同じプロセスの `tokio::spawn`。
- **Api / Federation**: 専用ポートで待ち受ける。`REDIS_URL` があれば `RedisJobQueue`、無ければ `InMemoryJobQueue`（split-role でこれだと他プロセスにジョブが届かない）。
- **Worker**（`run_standalone_worker`）: HTTP を持たずジョブを消費する。
- **Firehose**: 購読者がいないので空の `StreamHub` を使う。

DB プールの上限（`DB_MAX_CONNECTIONS` 未設定時）は `db::recommended_max_connections(extra)` が「`available_parallelism()`（HTTP の同時処理の近似。axum/tokio は同時実行数を制限しないので CPU コア数で見積もる）＋ `extra` ＋ 5」を50でクランプして決める（PostgreSQL 既定の `max_connections` 100 を1ロールで圧迫しないため）。All と Worker はジョブワーカー（`DEFAULT_MAX_CONCURRENT_JOBS` = 32並列）を持つので `extra` に32を、他は0を渡す。

同じイメージを `--role` 違いで複数コンテナに分けるか、1コンテナで `all` にするかは運用の選択で、コード上の分岐は `main.rs` の配線だけ。

- `docker-compose.yml`（split-role）: `db` / `redis`（ジョブキュー共有に必須）/ `api` / `federation-inbox` / `worker` / `atp-repo` / `frontend` / `nginx`（`docker/nginx.conf`）/ `tunnel`。`config-data` ボリュームで `secrets.toml` を共有する。`db` はコンテナ内DNSで足りるが、SSH トンネル経由の psql 用に `127.0.0.1:5432`（ループバックのみ）で公開する。
- `docker-compose.mono.yml`（単一コンテナ）: `db` / `seiran-server`（role=all）/ `frontend` / `nginx`（`docker/nginx.mono.conf`）/ `docker-gen`（`--scale seiran-server=N` を nginx に反映）/ `tunnel`。Redis は無い。`scripts/dev-up.sh` は `db` だけをこれで起動し、backend はホストで `cargo run` して `127.0.0.1:5432` に繋ぐ。
- `db` は両方とも `docker/Dockerfile.postgres`（`postgres:16-bookworm` に pg_bigm をソースビルド）で、`shared_preload_libraries=pg_bigm,pg_stat_statements` を渡す。`shared_preload_libraries` の変更は postmaster の再起動が要るので、コンテナ再作成までは `pg_stat_statements` ビューが使えない。
- `GET /health`（Api/All のみ、認証不要）: DB に `SELECT 1` できるかで 200/503 を返す外形監視用。
- `docker/Dockerfile`（seiran-server）は実プロセスを非 root（`seiran`、uid:gid 10001）で動かすが、イメージの `USER` は root のまま。`config-data`（`/app/config`）には root で書かれた既存ボリュームがマウントされうるが、Dockerfile の `chown` はイメージのレイヤーにしか効かず、空でないボリュームにはイメージの内容がコピーされないため。`docker/entrypoint.sh` が起動のたびに root で `/app/config` を `chown -R seiran:seiran` し、`gosu seiran` で降格して exec する（postgres 公式イメージと同じ構成）。他の書き込み先は `/tmp`（ffmpeg の一時ファイル）だけ。`docker/Dockerfile.frontend` は `node` ユーザーで、書き込みが無いので `USER` を固定している。`db`・`redis`・`nginx`・`cloudflared` は各イメージが自前で降格する。

## 4. 認証

認証の実体はローカル ID/PW（`auth::local::LocalAuthProvider`）の JWT だけで、MiAuth と Misskey 互換はその発行・受け渡しの窓口。外部認証プロバイダ連携は無い。

- パスワード: Argon2（既定パラメータ、`OsRng` の salt）
- トークン: HS256 の JWT。`sub` は `"local|{user_id}"`。自社ログイン（`generate_token`）・MiAuth（`generate_app_token`）とも `exp` を持たず無期限で、失効は個別の無効化（`app_tokens.revoked_at`）と一括失効（`users.token_valid_after`）で管理する。secret は `secrets.toml` の `jwt_secret`。クレームの `iat` が `token_valid_after` より前なら `extract_auth` が拒否する。パスワード変更・リセット・「全セッションからログアウト」で `token_valid_after` を現在時刻にする。`iat` の無い古いトークンは `token_valid_after` 未設定なら有効（導入時の強制全ログアウトを避けるため）。`extract_auth` は `exp` を無視してデコードする（`verify_token_ignoring_exp`）ので、以前の `exp` 付きトークンも有効。

**レート制限**:
- ログインと TOTP 検証は、暗号鍵を pepper にした keyed hash だけを `auth_attempt_log` に保存し、同一識別子に対する資格情報、同一資格情報に対する識別子を既定10分・5種類までに制限する。ウィンドウの起点は「10分前」「直近のパスワードリセット完了」「直近のログイン成功（`users.last_login_success_at`）」の最も新しい時刻で、ログイン成功で数え直しになる。拒否が同一IPで10分に5回起きると24時間 `auth_ip_blocks` で遮断する。
- 各値・Turnstile 鍵・登録のIP制限（既定60分・5件）は `site_settings` で管理する。Turnstile を設定するとログイン・登録で Siteverify を必須にする。
- クライアントIPは `ClientIp`（`cf-connecting-ip` → `x-real-ip` → `x-forwarded-for`、無ければ `None` で IP 系制限は素通り）。
- ロール別の上限（`rate_limit.rs` の `role_limit`、値は `site_settings`）: `user`/`emoji-editor` はメンション・返信・引用の宛先（DM除く）1時間30人、投稿1時間30通、新規フォロー24時間100人、リスト5本・各50人、検索1時間10回（スクロールのページングは数えない）、アップロード1ファイル10MB。`moderator` は投稿100通・フォロー300人・リスト30本/300人・検索50回・50MB。`admin` は対象外で API 全体の上限100MBだけ。

**TOTP**: 有効なユーザーはパスワード検証後に5分有効な用途限定 JWT を受け取り、`POST /api/auth/totp/verify` で TOTP かリカバリーコードを検証して通常 JWT を得る。用途限定 JWT はクレーム形状が違い、一般 API には使えない。シークレットは AES-256-GCM で暗号化、リカバリーコードは Argon2 ハッシュのみ保存する。両方失ったら、用途限定 JWT から登録メールへ1時間有効な解除リンクを送り、ワンタイムトークンの消費時に TOTP 設定を消す。管理者はユーザー管理で TOTP 状態とパスキー数を見て、TOTP を強制解除できる。

**パスキー**: WebAuthn の RP（RP ID は `LOCAL_DOMAIN`、origin は既定 `https://{LOCAL_DOMAIN}`、ローカル/E2E のみ `WEBAUTHN_ORIGIN` で上書き）。登録は resident key 必須・プラットフォーム認証器限定（`start_google_passkey_in_google_password_manager_only_registration`）で discoverable credential として保存するので、USB セキュリティキーは使えないが、ログイン画面で ID 入力なしにログインできる（`start_discoverable_authentication`/`identify_discoverable_authentication`）。チャレンジは `passkey_challenges` に保存し、5分で失効、完了時に原子的に削除する（認証開始時はユーザー未確定なので `user_id` は NULL 可）。成功時は署名カウンター等を更新して通常 JWT を発行する。パスキー自体がフィッシング耐性を持つので、パスワードと TOTP は求めない。

**MiAuth**: `GET /miauth/:session_id`（認可ページ）→ `POST /api/miauth/:session_id/authorize`（要 Bearer、無期限 JWT を発行）→ `POST /api/miauth/:session_id/check`（クライアントがポーリング）。セッションはプロセス内メモリ（`AppState.miauth_sessions`）。発行したトークンは `app_tokens` に記録し、設定画面で無効化するまで有効（Misskey クライアントは「取り消すまで有効」を前提にしている）。

**Misskey 互換**: `middleware::misskey_auth_bridge` が JSON ボディかクエリの `i` から `Authorization: Bearer` を合成する（既存のヘッダーを優先）。multipart（`drive/files/create`）は対象外なので、ハンドラが multipart の `i` を読む。

**DID 転入中の認可**: 転入フロー（`docs/account_migration.md`）は `submitPlcOperation` 成功までアカウントが無いので、ランダムトークンのハッシュを `at_migration_requests.request_token_hash` に保存し、生値を1回だけ返して、以降は `X-Migration-Token` で認可する。成功と同時に通常の `AuthResponse` を返す。

**転出元 API の認可**: `com.atproto.identity.*`/`server.checkAccountStatus`/`deactivateAccount` は ATP の accessJwt（`verify_atp_access_token`）で認可し、トークンの DID のアカウントだけを操作する（本文の DID は信用しない）。PLC 操作と `deactivateAccount` はメインパスワードでログインしたセッション（JWT の `privileged`）に限る（アプリパスワードを渡した第三者に DID を乗っ取らせないため）。DID 転出済み（`did_moved_out_at`）のアカウントは、書き込み系ハンドラ（投稿・リアクション・リポスト・フォロー・リスト・DM）が `require_not_did_moved_out()` を明示的に呼んで拒否する（凍結の判定と違い自動ではない）。読み取りは影響しない。

### API エラーレスポンス方針
`ApiError` は `{"code": "ERROR_CODE"}` を返す（平文は返さない）。Misskey 互換エンドポイントは `error: {code, message}` も付ける（`message` は `code` と同じ文字列。人間向けの文言はフロントの責務）。フロントの `getErrorMessage()` が `i18n/locales/{lng}/errors.json` で翻訳する（未知のコードは 5xx なら「サーバー応答なし」、それ以外はコード付きの汎用文言）。トークン失効（401 かつトークン保持中）なら `setUnauthorizedHandler()` 経由で自動ログアウトしてログイン画面へ誘導する。

## 5. ジョブキュー

`seiran-common::traits` に `Job`（enum）と `JobQueue` trait（`enqueue`/`enqueue_retry`/`dequeue_blocking`）を定義する。`WorkerEngine` は trait だけに依存する。

**バックエンド**（`create_job_queue(is_monolith)`）: `--role all` は常に `InMemoryJobQueue`。split-role は `REDIS_URL` があれば `RedisJobQueue`（優先度付き Sorted Set + `BZPOPMIN` + Lua による遅延リトライの昇格）、無ければ InMemory。

**主なジョブ**:
| Job | 用途 | 優先度 |
|---|---|---|
| `ActorHistorySync` | フォロー時の過去ログ取得（Bsky 300件、AP 30件） | 低 |
| `ApDelivery{actor_id, kind}` | AP 配送。`kind` は `PostToFollowers`/`DirectMessage`/`Announce`/`UndoAnnounce`/`DeleteNote`/`Reaction`/`UndoReaction`/`UpdateActor`/`DeleteActor` | 高 |
| `InboundActivityProcess` | 受信アクティビティの解析・保存（inbox は署名検証だけして 202 を返す） | 中 |
| `ActorMetadataResolve` | 相互申告マージの相手を取りに行く（`docs/protocols.md` 11節） | 低 |
| `AtpRepositoryPublish` | 使われていない（enqueue する箇所が無い） | — |
| `BskyVideoPoll{media_file_id}` | Bsky 動画パイプラインの完了待ち | — |
| `BskyPostCommitDeferred{actor_id, post_id, pending_media_file_id}` | 動画付き投稿の ATP コミットを動画の準備完了まで遅らせる | — |
| `ProxyFollowSync` | list-relay 仮想アクターの代理フォロー同期 | — |
| `AccountWithdrawUnfollowAll{actor_id, username}` | 退会時の一括アンフォロー | — |
| `BskyDmSend` / `BskyDmReactionAdd` / `BskyDmReactionRemove` / `BskyDmHide` | Bsky DM の送信・リアクション・「隠す」（`docs/protocols.md` 9節） | 高 |
| `RemoteFollowListSync{actor_id, direction}` | リモートの followers/following 全件取得（同期取得に失敗したときのフォールバック） | 低 |
| `RemoteActorResolve{uri}` | 未登録の actor URI を解決して upsert（フォロー関係は作らない） | 低 |
| `DmRecipientResolve{post_id, uri}` | Fedi 受信 DM の `to` にいるリモートアクターを `post_recipients` に加える | 低 |
| `RemoteInstanceInfoResolve{domain}` | リモートインスタンスの nodeinfo を `remote_instance_meta` にキャッシュ | 低 |
| `RemoteProfileRefresh{actor_id}` | リモートアクターのアバター・バナー・表示名等の再取得（表示時再検証） | 低 |
| `AlsoKnownAsVerify` / `RemoteAlsoKnownAsSync` | 「別のアカウント」の相互検証・同期（表示時再検証） | 低 |
| `BridgeUserLinkResolve{actor_id}` | ブリッジユーザーの実ユーザーへのリンク解決（解決済みなら何もしない） | 低 |
| `BskyListMembershipResolve{list_uri}` | リモート Bsky リストのメンバーを24時間キャッシュ（threadgate `#listRule`） | 低 |
| `FetchBridgeOriginal{bridge_post_id, target_uri, protocol}` | ブリッジポストの元ポストを取りに行く | 中 |
| `FollowImportProcess{request_id}` | フォローインポート（自己再 enqueue 型） | 低 |
| `MigrationFetchRepo` / `MigrationRequestPlcSignature` | DID 転入: リポジトリ取得・ステージング、確認コード送付の要求 | 中 |
| `MigrationImportProcess` / `MigrationImportFollows` | DID 転入: ステージング済みデータの実体化、フォロー関係の反映（自己再 enqueue 型） | 低 |
| `MigrationDeactivateSource` | DID 転入: 移行元アカウントの無効化（ベストエフォート） | 低 |

**自己再 enqueue 型**: 大量の対象を1件ずつ処理し、進捗を DB に反映しつつキャンセル可能にするパターン。対象を DB テーブル（状態列つき）に置き、ジョブは「未処理を1件処理して自分を再度積む」を繰り返し、対象が尽きるかリクエストが `running` でなくなったら止まる。レート制限等で一時的に進めないときは、エラーで WorkerEngine のバックオフに任せず、`enqueue_retry` で一定時間後に自分を積み直す（バックオフは DB 接続エラー等の本当の失敗用。`FollowImportProcess` は5分間隔）。
- `FollowImportProcess`: `follow_import_items` の `pending` を処理する。各行はカンマ区切りの1列目を識別子として読む（Misskey のフォローエクスポート `id,withReplies` 対応）。
- `MigrationImportProcess`: 未取り込みのレコード → blob の順に実体化し、尽きたら `deactivating_source` に進めて `MigrationDeactivateSource` と `MigrationImportFollows` を積む。
- `MigrationImportFollows`: `status` を見ず、未反映の `app.bsky.graph.follow` 行の有無だけで続けるか決める（転入完了を待たない結果整合処理。レート制限も適用しない）。

フォロー作成の実処理は `follow_exec::execute_follow`（`AppState` 非依存）で、API ハンドラ（`AppState::follow_exec_config()`）とジョブ（`JobContext::follow_exec`）が共有する。ターゲット文字列の種別判定は `follow_target::classify_follow_target`。既にフォロー済みでも成功を返し、`FollowOutcome.already_following` で区別する（インポートの進捗表示が「成功」と「既存」を分けて数える）。

**起動時リカバリ**: InMemory キューのリトライ待ちはプロセスの再起動で消えるので、DB に「未完了」の状態を持つジョブは、`spawn_startup_tasks` が起動のたびに無条件で積み直す。

| ジョブ | 未完了の条件 | 関数 |
|---|---|---|
| `FollowImportProcess` | `follow_import_requests.status='running'` | `resume_running_follow_imports` |
| `AccountWithdrawUnfollowAll` | `actors.withdrawn_at IS NOT NULL` かつ `follows` に残存行 | `resume_account_withdraw_unfollow_all` |
| `BskyVideoPoll` | `media_files.bsky_video_status = 'pending'` | `resume_bsky_video_poll` |
| `BskyPostCommitDeferred` | `posts.pending_bsky_media_file_id IS NOT NULL AND at_uri IS NULL` | `resume_bsky_post_commit_deferred` |
| DID 転入の各ジョブ | `at_migration_requests.status` ごとに振り分け | `resume_running_migrations` |
| `MigrationImportFollows` | 未反映の follow 行がある `request_id` | 同上（`list_request_ids_with_pending_follow_materialization`） |

「最後の進捗から一定時間経ったものだけ」等に絞ると、見積もり次第で本当に止まった処理を拾い損ねるので絞らない。

**重複実行の排除（`advisory_lock`）**: 無条件の積み直しは、動いている処理と重複しうる（split-role の複数レプリカや Redis に残ったジョブでも同様）。上記のジョブは開始時に `advisory_lock::try_acquire(pool, key)`（`pg_try_advisory_lock`）を試み、取れなければ何もせず終わる（積み直しもしない。動いている方が続ける）。
- キーは `request_id`・`actor_id`・`media_file_id`・`post_id`。採番元が違うキー同士は 64bit で偶然衝突する確率を無視している。ただし転入の `MigrationImportProcess` と `MigrationImportFollows` は同じ `request_id` で同時に積まれ必ず衝突するので、後者は `-request_id` を使う。
- advisory lock はセッションスコープなので、`try_acquire` は確保した1本の接続を返し、呼び出し側はそれを `release` に渡す。
- 次のジョブを積むジョブは、必ず unlock の後に積む。先に積むと、別ワーカーがすぐ取り出してロック取得に失敗し、積み直さずにチェーンが途切れる。
- 排他は同じリクエスト内の重複だけを防ぐ。インポート中の手動フォローとのレート制限の TOCTOU は実害が小さいので対象外。

**`BskyPostCommitDeferred` のペイロード**: `actor_id`/`post_id`/`pending_media_file_id` だけを持ち、本文・作成時刻・返信先はハンドラが `posts` から読み直す（起動時リカバリで `post_id` だけから再現できるように）。返信先の root/parent は `reply_to_post_id` の投稿の `at_uri`/`at_cid` を両方に使う（`resolve_reply_context` と同じ規約）。`pending_media_file_id` は投稿作成時に `posts.pending_bsky_media_file_id` に保存し、コミット成功後に NULL に戻す。

**表示時再検証**: 外部の状態に依存する値は、表示のたびに外部を取得すると遅く相手にも負荷がかかり、一度きりでは変化に追随できない。そこで表示は DB のキャッシュ値を即座に返し、表示のたびに低優先度の再検証ジョブを積んでキャッシュを更新する（リロードする頃には新しくなっている）。
- 1回で他のジョブを大量に積む重いジョブ（`RemoteFollowListSync` → 多数の `RemoteActorResolve`）を表示のたびに積むと、同じ優先度の他のジョブが進まなくなる。積む場所が1つなら、その層にプロセス内のクールダウン（`DashMap`）を置く（`AppState::remote_follow_sync_recent`、10分）。
- 積む場所が API 層とワーカー層にまたがるなら、ジョブモジュール自体にプロセス内 `static` のクールダウンを置き、両方がそれを経由する（`remote_actor_resolve::should_enqueue`、1時間）。解決できない URI はDBに入らないので、これがネガティブキャッシュも兼ねる。

**並列・排他制御**: ジョブ全体の同時実行数（`Semaphore`、既定32）、ドメイン単位の同時接続数（最大2、リモートから取得するジョブ用。`JobContext::get_domain_semaphore`）、アクター単位の直列化（ATP コミットの順序保証）、指数バックオフ＋ジッターのリトライ。AP 配送の inbox ファンアウトは `fan_out_activity` が `buffer_unordered` で最大8並列（追加の `tokio::spawn` はしない）。

## 6. 検索セッション管理

HTTP はステートレスで、フロントが検索画面を閉じたことをバックエンドは知れない。そこでメモリ上に「10分の砂時計」としてセッションを持つ。

```rust
pub struct SearchSession {
    pub query: String,
    pub appview_cursor: Option<String>,          // AppViewの次回カーソル
    pub unreturned_appview_posts: Vec<Post>,      // 取得済み未返却バッファ
    pub last_accessed_at: DateTime<Utc>,          // 寿命延長の主軸
    pub appview_exhausted: bool,
}
```

- 寿命はアクセスのたびに延びる10分。
- `SessionStore` trait。実装は `InMemorySessionStore`（`dashmap`）のみ（Redis 版は未実装、`docs/roadmap.md`）。

**ブレンド**（ID ベースの Misskey 互換要求と AppView のカーソルを翻訳する）:
1. **初回**: ローカルDB と AppView（`searchPosts`）を `tokio::join!` で30件ずつ取得し、AppView 分をDBに入れて統一IDを付けてから ID 降順でマージして上位30件を返す。残りはセッションのバッファへ。
2. **過去掘り**（`untilId`）: バッファが `limit` 未満なら AppView から追加取得し、ローカルからも取得して再ブレンドする。
3. **未来掘り**（`sinceId`）: ローカルDBだけ（通過した AppView 投稿は保存済みなので取りこぼさない）。
4. **セッション消滅時**: エラーにせず通常のローカル検索に落とす。

検索式は AppView にはそのまま渡し、ローカルには共通の AST に変換する。空白/`+` は AND、`OR` は OR、先頭 `-` は NOT、引用句と括弧が使える（括弧の不足は入力端で補う）。`from:`・`mentions:`・`domain:`・`since:`・`until:` をローカルにも適用し、`from:me`/`mentions:me` は自分に解決する。ローカル/Fedi 投稿は言語宣言が保証されないので `lang:` は常に TRUE。SQL は AST からプレースホルダー付きで生成し、LIKE のメタ文字をエスケープする。

マージ・降順ソート・重複排除・返却分とバッファへの分割は `search.rs::merge_sort_dedup_and_split()`（純粋関数）で、`InMemorySearchStore` とともに単体テストがある。

## 7. ストレージ・シークレット管理

**secrets.toml**（`seiran-common::secrets`）: `SEIRAN_CONFIG_DIR`（既定 `./config`）の `secrets.toml` を読み、無ければ生成して 0600 で保存する。中身は `jwt_secret`（256bit hex）、AT Protocol 用 P-256 鍵ペア、AP HTTP Signatures 用 RSA-2048 鍵ペア、`encryption_key`（AES-256-GCM）。`storage_providers.secret_key` 等は `encryption_key` で暗号化して保存する（`crypto.rs`）。

**S3 互換ストレージ**: `storage/selector.rs::select_provider()` が有効なプロバイダーを id 順に見て `capacity_mb` に収まる最初の1件を選ぶ。`storage/s3.rs` が PUT/DELETE、`media_probe.rs` が動画・音声のプローブ。

**動画の faststart 化**: `video/mp4`/`video/quicktime` は保存直前に `media_probe.rs::faststart_video()`（`ffmpeg -c copy -movflags +faststart`、再エンコードなし）を通す。`moov` アトムがファイル末尾にある mp4 は、ブラウザが `moov` を読むまで再生を始められない（Safari/iOS はほぼ再生不能）。sha256 の重複判定・ffprobe・Bsky 動画パイプラインへの提出は faststart 化前のバイト列で行う。失敗したら元のバイト列を保存する。

**画像アップロード**（`storage/image.rs::prepare_image()`）: 不要に劣化させないため候補を2つ作る。(1) `storage/exif.rs`（`img-parts`）で JPEG/PNG の Exif を Orientation だけに絞った無劣化の候補（画素は再エンコードしない）、(2) Orientation を画素に適用し `MediaKind` ごとの最大サイズにリサイズして WebP ロスレスにした候補。`media_store::store_image()` が両方の sha256+blurhash で重複を確認し、未登録なら小さい方を保存する。img-parts 非対応（静止画 WebP・AVIF・単一フレーム GIF 等）は (2) だけ。アニメーション画像（GIF/APNG/WebP）は元のバイト列を保存する。

**リモートメディアプロキシ（`GET /proxy?url=...`）**: フロントは別オリジンのアバター・添付・サムネイル・絵文字をこれに変換する。同一オリジンと、`/api/meta` の `internalMediaOrigins`（有効なストレージの公開URL。R2 等は別サブドメインのことが多い）は直接参照する（25MiB の上限にかからないように）。
- HTTP(S) のみ、資格情報・fragment を拒否、DNS 解決した全IPについて非公開IPを拒否。リダイレクト先も毎回検証し、5回・25MiB・20秒まで。画像・動画・音声以外は中継しない。
- 上流の `Content-Type` が許可リストに無い（`application/octet-stream` 等）ときは、アップロードと同じマジックバイト判定（`sniff_mime_type`）で判定し直し、それでも外れれば拒否する。
- `site_settings.media_proxy_url` があれば、Misskey の `instance.mediaProxy` と同じく `/proxy` まで含む完全なエンドポイントとして `{mediaProxyUrl}?url=...` で使う。
- 検証・取得は `handlers/media_proxy.rs::fetch_validated()` で、`/proxy` とリモート絵文字インポートが共有する。

**リモート絵文字カタログ（#73）**: AP 受信（本文・表示名・リアクション）で見つけたカスタム絵文字を `remote_emojis` に `upsert_seen` する（画像は取り込まない）。管理画面「絵文字」の「リモート」タブと、NoteCard の本文・リアクションの右クリックメニュー（管理者のみ、`EmojiContextMenu.tsx`）から、`EmojiImportDialog.tsx` → `POST /api/admin/emojis/remote/import` で `custom_emojis` に取り込む（`fetch_validated` → `prepare_image` → `store_image`）。

**未設定アバター（#211）**: アバターの無いローカル actor には、`actor_id` をシードに色相・目・口を決めた顔を `GET /api/avatars/:actor_id` で返す（生成と URL は `seiran-common::avatar`）。形式は SVG 非対応の Misskey クライアントのため PNG。内容は ID で決まるので `immutable` で長期キャッシュし、生成仕様を変えたら URL の `v` を上げる（現在 `v=5`）。フロント用 API・Misskey 互換 API とも同じ URL を返す。`/api` 配下に置くのは Cloudflare Tunnel の既存ルーティングを使うため。顔は目の間隔3段階（18/23/28）、口は原型の80%、笑顔の一種は上辺が直線のD型。ATP 側の扱いは `docs/protocols.md` 7節「代替アバターの ATP blob」。

**PWA**: `GET /manifest.webmanifest` が `site_settings`（`site_name`/`site_color`/`site_icon_sha256`）から Web App Manifest を毎回生成する（`display: standalone`）。アイコンは `GET /api/site-icon/:sha256/:size` がサイトアイコンを指定サイズの PNG にして返す。URL が sha256 を含むので `immutable` で長期キャッシュできる。アニメーション画像はリサイズせずそのまま返す（`image` crate がアニメーション PNG/WebP を書き出せず、演出を静止画にしないため）。`/favicon.ico` は `site_icon_sha256` があれば `/api/site-icon/:sha256/32`、無ければ `site_icon_url` へリダイレクトする。nginx は `/favicon.ico` と `/manifest.webmanifest` を API ロールへ振る。Service Worker は無い（オフライン動作・プッシュ通知は非対応）。

## 8. フロントエンド

React 18 + Vite + TypeScript。react-router-dom v7 を declarative mode（`<BrowserRouter>` とフック）で使い、データルーターは使わない。`frontend/src/`:

- `api/` — API クライアント。`core.ts`（`request`/`uploadFormData`/`ApiError`/`getErrorMessage()`）、`types.ts`（型と `Note` の正規化）、`webauthn.ts`、領域別モジュール（`auth`/`notes`/`users`/`admin`/`follows`/`lists`/`misc`）。`client.ts` はそれらを `api` に集約するだけ。フォロー操作は actor 行を持つ画面では `actorId` で呼ぶ（`follows.ts` の `followTargetOf`）。
- `components/layout/` — `AppShell`（3ペインの外枠）、`LeftNav`
- `components/note/` — `NoteCard`（タイムライン・詳細・プロフィール共通）、`PostComposer`、`ReactionChips`（チップのホバーでリアクター一覧をポップオーバー）、`ReactionPicker`、`HlsVideo`、`RichText` 等
  - `ReactionPicker` は `Modal` 内の `EmojiPickerPanel`。Unicode 絵文字データ（`unicode-emoji-json`）は `React.lazy` で遅延ロードする。カスタム絵文字は数千件になりうるので、`EmojiImage` は `hooks/useLazyVisible.ts`（root ごとに共有する `IntersectionObserver`）で視界外の `img` を描かず、グリッドは `PagedGrid` が200件ずつ段階的に描く。
  - 絵文字の検索索引（`lib/emojiAnnotations.ts`）は `emojibase-data` の生 JSON（1言語700〜800kB）を使わず、postinstall で `scripts/build-emoji-annotations.mjs` が生成する軽量 JSON（`src/generated/emoji-annotations/`、git 管理外、1言語170〜220kB）を言語ごとに動的 import する。
  - `RichText` は Markdown リンク・生URL・`@mention`・`#ハッシュタグ`・絵文字ショートコードを1パスでリンク等に変換する（`[#foo](リモートURL)` も `/tags/foo` に読み替える）。`EmojiText` はショートコード置換だけ。
  - 引用（`quote`）は1段だけ埋め込まれ、本文直下の枠付きカードで描く。カードは返信マーカー・ユーザー・CW・本文・時刻・添付・アンケート・リアクションを再利用し、引用元自身の引用は「引用あり」とだけ出す。
- `components/right/` — 右ペインのタブ内容（`NotificationsPanel`、`TrendsSearchPanel`、`FollowListPanel`、ポスト詳細の `AuthorPanel`・`ReplyThreadPanel`（`GET /api/notes/:id/replies` のフラット配列を `replyId`/`quoteId` でツリーにする）・`ReactionListPanel`・`RepostListPanel`（取り消し済みも含む）。`docs/ui_spec.md` 2.3節）
- `components/admin/` — 管理画面
- `components/dm/` — `RecipientPicker`（DM 宛先の chip 入力。Bsky と他プロトコルの混在を警告）
- アクター検索は用途別: `GET /api/actors/search` はリスト編集・DM 向けの表示名/全ハンドル部分一致、`GET /api/actors/suggest` は `ComposerEditor` 向けのハンドル前方一致で、応答の `target`（入力形式に応じたローカル短縮/Fedi/Bsky 表記）をそのまま挿入する。いずれも既知のアクターだけを探し、リモート取得はしない。
- `contexts/`
  - `AuthContext`: 起動時のセッション確認は `authSession.ts::resolveSession()`。`GET /api/auth/me` が明示的な 401 のときだけトークンを捨ててログアウトし、それ以外（再起動中の接続断・5xx）は 1s/2s/4s でリトライしても駄目ならトークンを保ったまま `sessionUnresolved` にする（バックエンドの再起動でログインが失われないように）。その間 `RequireAuth` はログイン画面へ誘導せず、`ServerUnavailableDialog` が5秒間隔で再試行して復旧したら閉じる。任意の API の 401 でも即ログアウトせず、同じ方針で `/auth/me` を1本にまとめて再確認する。
  - `ComposerContext`: 返信モーダルと、`openCompose(initialText)` による本文プリフィル付き投稿モーダル。
  - `RightPaneContext`: 右ペインのサブタブ状態と、ポスト詳細「前後のポスト」のスクロール位置（ポストIDごと）。
  - `StreamingContext`: WebSocket の集約。タイムライン新着はチャンネル購読（`subscribeChannel(spec, onNote)`）で受け、閲覧者権限付きの `GET /api/notes/:id` で完全な `NoteResponse` に補完してから渡す（`resolveStreamNote`。失敗時はストリームのペイロードを使う）。購読中のチャンネルは再接続時（`onOpen`）に全部 `connect` し直す。`HomePage` は表示タブが変わるたびに購読を切り替える。DM 新着は `registerDirectMessage` で振り分け、未読数 `dmUnreadCount` を LeftNav のバッジに渡す。`followAccepted` で `followStatusStore` を更新する。
  - `ToastContext`: トースト通知。
- `stores/` — モジュールスコープの `Map` + `useSyncExternalStore` の外部ストア。同じ対象が画面内の複数コンポーネントに同時に出るので、ローカル state で持たずここを購読する。
  - `followStatusStore.ts`（`userRelationshipStore` のファサード）: フォロー状態。キーは `lib/format.ts` の `profileQuery(username, domain)`。
  - `reactionStore.ts`: リアクション集計（キーはノートID）。WS のリアルタイム更新もここに書くので、`HomeFeedContext`（ブラウザバック時にタイムラインを再取得せず復元するための Note 配列キャッシュ）の集計が古くても表示は最新になる。
  - `pollVoteStore.ts`: アンケート票数（`pollUpdated`）。
- `pages/` — 画面単位のコンポーネント
- `i18n/` — `react-i18next` + `i18next-browser-languagedetector`。
  - 表示言語（`displayLanguages`、8言語: `en`/`ja`/`zh-Hant`/`zh-Hans`/`ko`/`es`/`de`/`fr`）とポスト言語（`postLanguages`、7言語。中国語は `zh` のみで `SUPPORTED_LANGUAGES` と一致）を別に持つ。`postLanguageBase()` が表示言語をポスト言語に丸める（投稿フォームの既定値・絵文字アノテーションの言語）。
  - 「自動」はブラウザ設定から判定し、`normalizeDetectedLanguage()` が `zh-TW`/`zh-HK`/`zh-MO` → `zh-Hant`、他の `zh` → `zh-Hans`、他言語は言語部分に正規化する（対応外は `en`）。`detection.convertDetectedLanguage` と `load: "currentOnly"`（`languageOnly` だと `zh-Hant` が `zh` に丸められる）にも使う。
  - 設定画面で明示的に選ぶと `localStorage` に記憶し、ログイン中は `users.language_preference` を優先する（`AuthContext` が適用）。
  - 翻訳は `i18n/locales/{言語}/{名前空間}.json` に分け、`import.meta.glob` でバンドルする。`i18n/index.test.ts` が全言語のキーと補間変数の一致を検査する。名前空間分割は、ユーザー製の言語ファイルを `i18n.addResourceBundle()` で追加する構想を見据えたもの。

3ペインのレイアウト仕様は `docs/ui_spec.md`。

**ローカル開発サーバーの標準運用**: agentic coding が主体なので、HMR 付きの dev サーバー（`npm run dev`、5173。StrictMode の effect 二重実行など開発ビルド固有の挙動がある）ではなく、`npm run build:watch`（変更のたびに本番相当のビルド）と `npm run preview`（`dist/` を配信、既定4174、`PREVIEW_PORT` で変更可）の2プロセスを常駐させ、常に本番相当のコードで確認する。バックエンドの `FRONTEND_ORIGIN`（OGP 注入・転送先）も既定でこの preview を指す。人手で UI を細かく調整するときは `FRONTEND_ORIGIN` を `http://localhost:5173` にして `npm run dev` を使ってよい。

**開発サーバーの健全性確認**: プロセスが生きていても正しく動いているとは限らない。
- `vite build --watch` は、この環境ではネイティブのファイル監視が信頼できず、変更検知だけが黙って止まることがある。`scripts/dev-up.sh` は `CHOKIDAR_USEPOLLING=true` を付けて起動する。疑わしければ `ls -la frontend/dist/assets/*.js` の更新時刻（ファイル名はコンテンツハッシュ入り）を編集時刻と比べ、古ければプロセスを止めて `CHOKIDAR_USEPOLLING=true npm run build:watch` を再実行する（preview は再起動不要）。
- バックエンド（`cargo run -p seiran-server`、3000）も `ps` ではなく `curl localhost:3000/` 等の実応答で確認する。
- 「生きているのに動きがおかしい」ときは `df -h` も見る。`target/` は肥大化しやすく、逼迫すると原因不明な壊れ方をする（`cargo clean` で回収できる）。

**開発用プロキシと Vite 内部パス**: `vite.config.ts` の dev サーバーは `/@:handle` をバックエンドへ転送するが、単純なプレフィックス一致だと `/@vite/client`・`/@react-refresh`・`/@fs/...`・`/@id/...` まで転送して白画面になるので、それらを除く正規表現にしている。

## 8.1 OGP (Open Graph) 対応

SPA の index.html には投稿・プロフィールごとの `<meta>` が無い。User-Agent で既知の bot だけを出し分けると未知のクローラーを取りこぼすので、`/notes/:id`・`/@:handle`（AP の `Accept` を除く）は常に、index.html の `<head>` に OGP を注入したものを返す。ブラウザはそのまま SPA が起動し、クローラーは `<meta>` だけを読む。

- `handlers/ogp.rs` が DB から投稿/アクターを取り、`state.frontend_origin`（Docker 既定 `http://frontend:5173`、ローカルは preview、`FRONTEND_ORIGIN` で変更可）から index.html を取って `<title>`・OGP・Twitter Card を注入する。`GET /notes/:id` は `get_note_ap` が `Accept` で AP の JSON-LD と出し分け、`GET /@:handle` はプロフィール専用。
- 見つからない・DB エラー時は注入せず index.html をそのまま返す（404 を返すと SPA が起動できず、「見つかりません」表示やリモートアクターの取得が動かない）。
- 可視性は通常の閲覧と同じ判定（`find_by_id_for_viewer` を viewer 無しで呼ぶ）。
- nginx と Vite の proxy は `/notes`・`/@`・`/announces` を常にバックエンドへ転送する。
- リポスト（Announce）の AP の URL は `/announces/:id`。ブラウザで開かれたら `get_announce_redirect` が `/notes/:id` へリダイレクトする（AP クライアント向けの Announce 応答は未実装で 404）。
- Docker では `fetch_spa_html` の Host ヘッダーがコンテナ名（`frontend`）になるので、`vite.config.ts` の `server`/`preview` の `allowedHosts` に `LOCAL_DOMAIN` と `"frontend"` の両方を入れる。無いと Vite が内部取得を弾き、`/notes/:id`・`/@:handle` の直接アクセスがすべて壊れる。

## 8.2 `/users/:username`（AP actor ID）の旧形式プロフィールURL互換

`/users/:username` は AP actor の `id` として恒久的に維持する。`/@handle` 形式の導入前はブラウザ向けのプロフィールURLでもあり、リモートに残るプロフィール記録がこれを actor URL として持っている。

- `actor.rs::actor_handler`: `Accept` に `activity+json`/`ld+json` を含まない（ブラウザ）なら `/@:username` へ 302。AP クライアントには Actor JSON-LD（`id`/`publicKey.owner` は `/users/:username` のまま、`url` は `/@:username`）。
- `webfinger.rs`: `resource` が `acct:user@domain` でも `https://{domain}/users/{username}` でも同じ応答を返す（キャッシュ済みの actor URL で再検証してくる実装がある）。
- nginx は `/users/` 配下を常にバックエンドへ転送する。

## 9. テストとCI

**CI**（GitHub Actions）:
- `Rust` job: `cargo fmt --all -- --check` と `clippy -- -D warnings`。
- `Frontend` job: 型チェック・lint（警告も失敗）・Vitest。
- `E2E` job（Node.js 20・Chromium・E2E 専用 PostgreSQL）: まず DB 結合テスト（`crates/seiran-api/tests/*_integration.rs`、`#[ignore]` 付き。`cargo test -p seiran-api --tests -- --ignored`）を同じ専用DBで実行し、`docker compose down -v` で DB を捨ててから全 Playwright テストを実行する（E2E は空のDBから始める前提）。結合テストのハーネス（`tests/support/mod.rs`）がマイグレーションとテストユーザー（`seiran1` 等、パスワード `seiranda`）を用意する。失敗時は `playwright-report`/`test-results` を7日間保存する。rust-cache は `Rust` job と `shared-key` で共有（E2E 側は読み取りのみ）、Postgres イメージは Buildx の GHA キャッシュ、Playwright のブラウザは `actions/cache`。
- `dtolnay/rust-toolchain@stable` は両 job とも `toolchain` のバージョンを固定する。固定しないと新しい stable のたびに clippy に lint が増え、コード変更なしに CI が落ちる。更新するときは両 job を揃える。

**E2E**（`e2e/`、`cd e2e && npm test`）: 外部の実サービス（Fedi/Bsky・PLC・Relay）とは通信せず、相手をすべてローカルのスタブに置き換える。ポートは開発サーバーと別（バックエンド3100・フロント5273、`e2e/ports.ts`）なので、開発サーバーを止めなくてよい。

- `playwright.config.ts` の `webServer` がスタブ PLC・スタブ AppView・スタブ Fedi・バックエンド・フロントを起動する。バックエンドには `PLC_DIRECTORY_BASE_URL`/`ATP_APPVIEW_URL` をスタブへ、`ATP_RELAY_URL` を存在しないポートへ、`CLOUDFLARE_*` を空に、`SEIRAN_ALLOW_PRIVATE_NETWORK=true`（スタブが 127.0.0.1 のため）、`SQLX_OFFLINE=true`（空の DB でコンパイル時検証が失敗しないよう `.sqlx/` を使う）を渡す。
  - 全 `webServer` の `reuseExistingServer` は `false` 固定（変更禁止）。`true` だとポートが空いていない場合に既存のサーバーへ相乗りし、実開発DBへのテストデータ混入や本物の plc.directory への誤登録を起こす。
  - Playwright は「webServer 起動 → globalSetup」の順なので、E2E 用 Postgres（`e2e/docker-compose.yml`、5433）の起動待ちはバックエンドの `command` の前段（`scripts/wait-for-db.ts`）に入れている。`global-setup.ts` は起動済みのバックエンドで初期管理者を作る（`users` が空だとフロントは常にセットアップ画面を出すため）。`globalTeardown` が DB を `down -v` で捨てる。
  - project は3つ（`workers` は CI 3・ローカル4）。大半は `main`。`storage_providers` に触れる spec（スタブ S3 の登録が競合する）は `storage-serial`（project 内 `workers: 1`、`main` と並行）、`site_settings` やスタブのグローバル状態に触れる spec は `globals-serial`（`main`・`storage-serial` の後に排他で実行）。
- `fixtures/stub-plc-server.ts`: plc.directory のスタブ（`tsx` で直接実行）。genesis op を保持し、DID 文書にして返す。
- `fixtures/stub-appview-server.ts`: AppView のスタブ。主要エンドポイントに空の結果を返す。
- `fixtures/stub-fedi-server.ts`: リモート AP アクターのスタブ。正規の HTTP Signatures で署名した Follow を送り、フォロー後に配送された投稿・返信・リポストを自分の inbox で記録する。
- `fixtures/api-helpers.ts`: 前提データ（相手ユーザー等）は UI ではなく API で作り、`seedAuth()` で localStorage にトークンを仕込んでログイン操作を省く（`login.spec.ts` だけはフォームを操作する）。
- DB はテスト実行全体で共有するので、各テストは一意なプレフィックス＋タイムスタンプでユーザーを作る。手動検証用の `seiran{n}` とは別DB。
- フロントはブラウザのロケールで言語を決めるので `use.locale` を `ja-JP` に固定している。
- Cloudflare DNS（TXT 自動登録）と、本物の Jetstream が要る Bsky 受信系は対象外。

## 10. 環境変数

| カテゴリ | 変数 |
|---|---|
| ドメイン | 自ホストドメインは `instance_domain` テーブル（一度確定したら不変、`LocalDomain`/`InstanceDomainRepository`）から起動時に読む。未確定なら `LOCAL_DOMAIN` の値で確定させる。どちらも無い新規インストールでは、初回セットアップ（`POST /api/setup`）でリクエストの生の `Host` ヘッダーから確定する（`GET /api/setup/status` が候補 `domain_candidate` を返し、フロントが確認後そのまま送り返し、サーバーは送信時の `Host` と一致するか検証する。不一致は `DOMAIN_MISMATCH`）。`Host` が無い・`localhost`・IP 直打ちなら「シングルホストモード」（連合なし、DID を持たないローカルユーザー、`actors.domain='localhost'`） |
| 起動ポート | `PORT`（既定3000）、`FEDERATION_INBOX_PORT`（既定3001） |
| データベース | `POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB`、`DB_HOST`/`DB_PORT`（既定 `localhost`/5432。Docker では `DB_HOST=db`）、`DB_MAX_CONNECTIONS`（未設定ならロールごとに算出、3節）。完成済みの `DATABASE_URL` は使わない（`db::get_db_pool`） |
| ジョブキュー | `REDIS_URL`（split-role 用。`--role all` では不要） |
| シークレット | `SEIRAN_CONFIG_DIR`（既定 `./config`）。JWT secret 等は `secrets.toml` で自動生成する |
| 外部サービス | `TUNNEL_TOKEN`（Cloudflare Tunnel）、`CLOUDFLARE_API_TOKEN`/`CLOUDFLARE_ZONE_ID`（ハンドル検証の TXT 自動作成。未設定なら `.well-known` 方式のみ）、`ATP_RELAY_URL`（`requestCrawl` 先、カンマ区切り、既定 `https://bsky.network`）、`PLC_DIRECTORY_BASE_URL`（既定 `https://plc.directory`）、`ATP_APPVIEW_URL`（既定 `https://api.bsky.app`）、`SEIRAN_ALLOW_PRIVATE_NETWORK`（`true` で連合用クライアントの非公開IP拒否を無効化。E2E 専用） |
| 開発・一回限り | `WEBAUTHN_ORIGIN`（ローカル/E2E のパスキー origin）、`ATP_BACKFILL_UNSET_AVATAR_PROFILES_ONCE`、`BSKY_FOLLOWER_POLL_INTERVAL_SECS` |
| SMTP | 環境変数ではなく `site_settings`（管理 API）で設定する |

## 11. 横断機能の構成メモ

- **「開く」**（`handlers/open_target.rs`）は薄いオーケストレーション層で、Bsky は `seiran-common::atp`、AP Actor は `target_resolve`、AP 投稿は受信 Create ジョブを再利用する。外部の ActivityStreams 文書の取得はメディアプロキシと同じ検証（DNS 固定・非公開IP拒否・リダイレクト再検証）を通す。フロントの `OpenTargetDialog` は QR を同期認識し、重い OCR Worker は読み取り開始後に動的 import する。
- **通報**は `ReportModal` と `POST /api/reports` でローカル・Fedi・Bsky を統一し、管理画面の「通報」タブで台帳閲覧・クローズ・内部コメント・凍結/投稿削除・リモート転送を行う（転送の仕様は `docs/protocols.md`「通報配送」）。
- **Snowflake ID** はブラウザで精度を失わないよう、API では文字列で受け渡す。
