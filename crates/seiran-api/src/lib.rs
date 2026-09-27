//! seiran-api — REST API / 認証 / タイムライン / XRPC を提供するライブラリ。
//!
//! バイナリは `seiran-server` が `--role api`（または `all`）で起動する。
//! ここでは AppState 構築（[`init_state`]）・ルーター構築（[`router`]）・
//! 起動時タスク（[`spawn_startup_tasks`]）を公開し、実際の serve は呼び出し側が行う。

pub mod cloudflare;
pub mod error;
pub mod handlers;
pub mod mailer;
pub mod middleware;
pub mod rate_limit;
mod routes;
pub use routes::router;
pub mod search;
pub mod streaming;

use dashmap::DashMap;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use webauthn_rs::prelude::{Url, Webauthn, WebauthnBuilder};

use seiran_common::repository::{
    ActorRepository, AlsoKnownAsRepository, AppTokenRepository, AtpPreferencesRepository,
    AtpReadRepository, AtpSessionRepository, AuthRateLimitRepository, BlockRepository,
    DmRepository, EmailChangeRepository, EmailShortCodeRepository, EmailVerificationRepository,
    EmojiRepository, FollowImportRepository, FollowRepository, HashtagRepository,
    InstanceDomainRepository, LinkResolutionRepository, ListRepository, MuteRepository,
    NotificationRepository, PasswordResetRepository, PgActorRepository, PgAlsoKnownAsRepository,
    PgAppTokenRepository, PgAtpPreferencesRepository, PgAtpReadRepository, PgAtpSessionRepository,
    PgAuthRateLimitRepository, PgBlockRepository, PgDmRepository, PgEmailChangeRepository,
    PgEmailShortCodeRepository, PgEmailVerificationRepository, PgEmojiRepository,
    PgFollowImportRepository, PgFollowRepository, PgHashtagRepository, PgInstanceDomainRepository,
    PgLinkResolutionRepository, PgListRepository, PgMuteRepository, PgNotificationRepository,
    PgPasswordResetRepository, PgPinnedPostsRepository, PgPostRepository, PgReactionRepository,
    PgRelayRepository, PgRemoteEmojiRepository, PgRemoteInstanceMetaRepository,
    PgRepostMuteRepository, PgTotpRepository, PgUserRepository, PinnedPostsRepository,
    PostRepository, ReactionRepository, RelayRepository, RemoteEmojiRepository,
    RemoteInstanceMetaRepository, RepostMuteRepository, TotpRepository, UserRepository,
};
use seiran_common::{
    job_priority, ApClient, ApDeliveryKind, AtpCommitEvent, AtpCommitService, Job, JobQueue,
    LocalAuthProvider, MediaFileRepository, PgMediaFileRepository, PgSiteSettingsRepository,
    PgStorageProviderRepository, S3StorageClient, Secrets, SiteSettingsRepository,
    StorageProviderRepository,
};

use handlers::miauth::MiAuthSession;
use search::InMemorySearchStore;
use streaming::StreamHub;

// =====================================================================
// アプリケーション状態
// =====================================================================

#[derive(Clone)]
pub struct AppState {
    /// リポジトリ層（SQL アクセスはここを経由する）
    pub actors: Arc<dyn ActorRepository>,
    /// プロフィールの「別のアカウント」（alsoKnownAs、AP Moveの語彙をプロフィール表示・
    /// 相互検証用途に転用したseiran独自拡張）。
    pub also_known_as: Arc<dyn AlsoKnownAsRepository>,
    pub users: Arc<dyn UserRepository>,
    pub posts: Arc<dyn PostRepository>,
    pub follows: Arc<dyn FollowRepository>,
    /// フォローインポート（設定画面から改行区切りのID一覧を貼り付けて一括フォロー）の
    /// 進捗管理（`follow_import_requests`/`follow_import_items`）。
    pub follow_imports: Arc<dyn FollowImportRepository>,
    /// ブロック関係（Bsky準拠：フォロー強制解除＋相互完全非表示）。
    pub blocks: Arc<dyn BlockRepository>,
    /// ミュート関係（ローカル効果のみ、AP/ATP配送なし）。
    pub mutes: Arc<dyn MuteRepository>,
    /// リポストミュート関係（対象ユーザーのリポストのみをタイムラインから隠す独立フラグ、
    /// ローカル効果のみ、AP/ATP配送なし）。
    pub repost_mutes: Arc<dyn RepostMuteRepository>,
    /// 発行済みアプリトークン（MiAuth 経由、#60）の一覧・無効化リポジトリ。
    pub app_tokens: Arc<dyn AppTokenRepository>,
    pub atp_repo: Arc<dyn AtpReadRepository>,
    /// AT Protocol セッション認証（アプリパスワード・リフレッシュトークン）リポジトリ。
    pub atp_sessions: Arc<dyn AtpSessionRepository>,
    /// AT Protocol クライアント設定（`app.bsky.actor.getPreferences`等）リポジトリ。
    pub atp_preferences: Arc<dyn AtpPreferencesRepository>,
    /// リアクション（絵文字リアクション・いいね）リポジトリ。
    pub reactions: Arc<dyn ReactionRepository>,
    /// ピン留めポスト（ローカルユーザーの pin/unpin 操作結果 + リモートアクターの同期結果の共通ストア）。
    pub pinned_posts: Arc<dyn PinnedPostsRepository>,
    /// 通知（フォロー・リアクション等）の永続化リポジトリ。
    pub notifications: Arc<dyn NotificationRepository>,
    /// ダイレクトメッセージ（DMセッション一覧・履歴・既読状態）の永続化リポジトリ。
    pub dm: Arc<dyn DmRepository>,
    /// deliver_post_to_ap_followers（seiran-common）が &PgPool を要求するため保持。
    /// 将来 FollowerRepository へ移行したら削除する。
    pub db: PgPool,
    pub local_auth: Arc<LocalAuthProvider>,
    pub miauth_sessions: Arc<RwLock<HashMap<String, MiAuthSession>>>,
    pub local_domain: seiran_common::LocalDomain,
    pub instance_domain: Arc<dyn InstanceDomainRepository>,
    /// リモートインスタンス（Fedi）のnodeinfoキャッシュ（#NoteCardリモートサーバー表示）。
    pub remote_instance_meta: Arc<dyn RemoteInstanceMetaRepository>,
    /// bio/profile_fields中のURL解決結果のキャッシュ（#リンク解決）。
    pub link_resolutions: Arc<dyn LinkResolutionRepository>,
    /// OGP対応（`handlers::ogp`）で SPA の index.html を取得する先。未設定時は Docker
    /// 構成のデフォルト（`http://frontend:5173`）を使う。
    pub frontend_origin: String,
    pub secrets: Arc<Secrets>,
    pub atp_service: Arc<AtpCommitService>,
    pub http_client: Arc<reqwest::Client>,
    pub ap_client: Arc<ApClient>,
    pub cloudflare: Option<Arc<cloudflare::CloudflareClient>>,
    pub storage_providers: Arc<dyn StorageProviderRepository>,
    pub media_files: Arc<dyn MediaFileRepository>,
    pub site_settings: Arc<dyn SiteSettingsRepository>,
    /// URLカード埋め込みプレーヤー（oEmbed discovery）の許可ドメイン判定。TTLキャッシュ済み。
    pub oembed_whitelist: Arc<seiran_common::oembed_whitelist::OembedWhitelist>,
    pub search_store: Arc<InMemorySearchStore>,
    /// リアルタイム更新（#37）のストリーミングハブ。
    pub stream_hub: Arc<StreamHub>,
    /// 絵文字インポートジョブの進捗状態（#50）。job_id → ImportJobStatus。
    pub emoji_import_jobs: Arc<DashMap<String, handlers::admin::emoji_import::ImportJobStatus>>,
    /// `RemoteFollowListSync` 重複投入防止用クールダウン（#229）。
    /// (actor_id, direction) → 直近enqueue時刻。プロフィールリロードのたびに同一ジョブが
    /// 積まれ続け、低優先度ジョブキューが埋め尽くされる問題への対処。プロセス内のみで
    /// 完結する簡易ガードのため、split-role構成でAPIプロセスが複数台ある場合は台数分だけ
    /// クールダウンが緩む（許容: 根本的な多重防止はWorker側のジョブ重複排除で行うべきだが、
    /// まずは支配的なケース＝同一プロセスへの連続リロードを塞ぐ）。
    pub remote_follow_sync_recent: Arc<DashMap<(i64, String), std::time::Instant>>,
    /// 非同期ジョブキュー（AP配送・Bsky動画パイプライン結合等）。`all` ロールでは
    /// `seiran-federation-worker`のWorkerEngineと同一インスタンスを共有する。
    pub job_queue: Arc<dyn JobQueue>,
    /// リスト機能（#63）: 誰にもフォローされていないリモートFediユーザーの投稿を
    /// 受信するための代理フォロー用仮想アクター（list-relay）の actor_id。
    pub lists: Arc<dyn ListRepository>,
    /// ハッシュタグ（ポスト⇔タグのm:n関係の永続化、ハッシュタイムライン、ホーム画面ピン留め）。
    pub hashtags: Arc<dyn HashtagRepository>,
    pub system_proxy_actor_id: i64,
    /// パスワードリセットフロー（`password_resets` テーブル）。
    pub password_resets: Arc<dyn PasswordResetRepository>,
    /// 認証ブルートフォース対策（`auth_attempt_log` / `auth_ip_blocks`、#223）。
    pub auth_rate_limits: Arc<dyn AuthRateLimitRepository>,
    /// 新規登録時のメール確認フロー（`email_verifications` テーブル）。
    pub email_verifications: Arc<dyn EmailVerificationRepository>,
    /// 設定画面からのメールアドレス変更フロー（`email_changes` テーブル、#59）。
    pub email_changes: Arc<dyn EmailChangeRepository>,
    /// メール短命コード（ATPセッション2FA・PLCオペレーション署名確認、`email_short_codes`テーブル）。
    pub email_short_codes: Arc<dyn EmailShortCodeRepository>,
    /// カスタム絵文字（`custom_emojis` テーブル）。
    pub emojis: Arc<dyn EmojiRepository>,
    /// リモートカスタム絵文字カタログ（`remote_emojis` テーブル、#73）。
    pub remote_emojis: Arc<dyn RemoteEmojiRepository>,
    /// Fediverseリレー参加先（`fediverse_relays` テーブル、#140）。
    pub relays: Arc<dyn RelayRepository>,
    /// TOTP（二段階認証）設定・リカバリーコード・メール経由の強制解除リクエスト（#65）。
    pub totp: Arc<dyn TotpRepository>,
    pub webauthn: Arc<Webauthn>,
}

/// `enqueue_remote_follow_list_sync` の重複投入防止クールダウン（#229）。
/// この時間内の同一 (actor_id, direction) への再投入は無視する。
const REMOTE_FOLLOW_SYNC_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(600);

impl AppState {
    /// `seiran_common::follow_exec::execute_follow` に渡す設定を組み立てる。
    /// フォローインポートジョブ（`JobContext::follow_exec`）と全く同じ実処理を
    /// API ハンドラからも呼べるようにするための橋渡し（軽量な Arc クローンのみ）。
    pub fn follow_exec_config(&self) -> seiran_common::FollowExecConfig {
        seiran_common::FollowExecConfig {
            actors: Arc::clone(&self.actors),
            follows: Arc::clone(&self.follows),
            blocks: Arc::clone(&self.blocks),
            notifications: Arc::clone(&self.notifications),
            atp_service: Arc::clone(&self.atp_service),
            stream_hub: Arc::clone(&self.stream_hub),
            local_domain: self.local_domain.clone(),
            ap_private_key_pem: self.secrets.ap_private_key_pem.clone().unwrap_or_default(),
        }
    }

    /// リプライ/引用/リポストの`pending`参照解決（`resolve_pending_reference_with_timeout`、#233）
    /// が必要とする`InboxContext`を組み立てる。ジョブワーカー専用だった同構造体をAPI
    /// ハンドラからも使えるようにする橋渡し（軽量なArcクローンのみ）。
    pub fn inbox_context(&self) -> seiran_common::queue::worker::InboxContext {
        seiran_common::queue::worker::InboxContext {
            db_pool: self.db.clone(),
            actor_repo: Arc::clone(&self.actors),
            follow_repo: Arc::clone(&self.follows),
            block_repo: Arc::clone(&self.blocks),
            post_repo: Arc::clone(&self.posts),
            reaction_repo: Arc::clone(&self.reactions),
            notification_repo: Arc::clone(&self.notifications),
            hashtag_repo: Arc::clone(&self.hashtags),
            remote_emoji_repo: Arc::clone(&self.remote_emojis),
            list_repo: Arc::clone(&self.lists),
            local_domain: self.local_domain.clone(),
            ap_private_key_pem: self.secrets.ap_private_key_pem.clone().unwrap_or_default(),
            stream_hub: Arc::clone(&self.stream_hub),
            queue: Arc::clone(&self.job_queue),
            atp_service: Arc::clone(&self.atp_service),
        }
    }

    /// list-relayプロキシアクターの署名鍵（キーID, 秘密鍵PEM）。Authorized Fetch
    /// （secure mode）対応のGET（`ApClient::fetch_actor_signed`/`fetch_ap_collection_uris`等）に使う。
    /// 秘密鍵未設定時は`None`（呼び出し側は未署名フェッチへフォールバックする）。
    pub fn system_signing_key(&self) -> Option<(String, String)> {
        let pem = self.secrets.ap_private_key_pem.as_deref()?;
        Some(seiran_common::system_actor::system_signing_key(
            &self.local_domain,
            pem,
        ))
    }

    /// AP 配送ジョブを積む。配送の実行・リトライは Worker（`jobs::ap_delivery`）が担う。
    /// enqueue 失敗はログのみ（投稿等の主処理は成功済みのため呼び出し元へは伝播しない）。
    pub async fn enqueue_ap_delivery(&self, actor_id: i64, kind: ApDeliveryKind) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::ApDelivery { actor_id, kind }, job_priority::HIGH)
            .await
        {
            tracing::error!(
                "[job] ApDelivery enqueue 失敗 (actor_id={}): {}",
                actor_id,
                e
            );
        }
    }

    /// 過去ログ同期ジョブ（ActorHistorySync）を積む。
    pub async fn enqueue_actor_history_sync(&self, ap_uri: Option<String>, at_did: Option<String>) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::ActorHistorySync { ap_uri, at_did }, job_priority::LOW)
            .await
        {
            tracing::error!("[job] ActorHistorySync enqueue 失敗: {}", e);
        }
    }

    /// フォローインポートジョブ（自己再enqueue型、`jobs::follow_import`）を積む。
    /// インポート開始時・1件処理完了後の両方から呼ばれる。
    pub async fn enqueue_follow_import_process(&self, request_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::FollowImportProcess { request_id }, job_priority::LOW)
            .await
        {
            tracing::error!(
                "[job] FollowImportProcess enqueue 失敗 (request_id={}): {}",
                request_id,
                e
            );
        }
    }

    /// 既存DID転入フロー（`docs/account_migration.md`）: PDS Aからの`getRepo`/`listBlobs`取得を積む。
    pub async fn enqueue_migration_fetch_repo(&self, request_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::MigrationFetchRepo { request_id }, job_priority::NORMAL)
            .await
        {
            tracing::error!(
                "[job] MigrationFetchRepo enqueue 失敗 (request_id={}): {}",
                request_id,
                e
            );
        }
    }

    /// 既存DID転入フロー: PDS Aへの`requestPlcOperationSignature`呼び出しを積む。
    /// `enqueue_migration_fetch_repo`完了直後にジョブ自身が積む。
    pub async fn enqueue_migration_request_plc_signature(&self, request_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::MigrationRequestPlcSignature { request_id },
                job_priority::NORMAL,
            )
            .await
        {
            tracing::error!(
                "[job] MigrationRequestPlcSignature enqueue 失敗 (request_id={}): {}",
                request_id,
                e
            );
        }
    }

    /// 既存DID転入フロー: データ取り込み（`posts`/`atp_records`/`atp_blocks`/`media_files`への
    /// 実体化、自己再enqueue型）を積む。
    pub async fn enqueue_migration_import_process(&self, request_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::MigrationImportProcess { request_id },
                job_priority::LOW,
            )
            .await
        {
            tracing::error!(
                "[job] MigrationImportProcess enqueue 失敗 (request_id={}): {}",
                request_id,
                e
            );
        }
    }

    /// 既存DID転入フロー: 移行元PDS Aアカウントの無効化（ベストエフォート）を積む。
    pub async fn enqueue_migration_deactivate_source(&self, request_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::MigrationDeactivateSource { request_id },
                job_priority::LOW,
            )
            .await
        {
            tracing::error!(
                "[job] MigrationDeactivateSource enqueue 失敗 (request_id={}): {}",
                request_id,
                e
            );
        }
    }

    /// 既存DID転入フロー: フォロー関係の復元（`follows`テーブルへの反映、自己再enqueue型）を積む。
    /// `at_migration_requests.status`とは独立した結果整合処理のため、起動時リカバリも
    /// ステータス起点ではなく`list_request_ids_with_pending_follow_materialization`で判定する。
    pub async fn enqueue_migration_import_follows(&self, request_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::MigrationImportFollows { request_id },
                job_priority::LOW,
            )
            .await
        {
            tracing::error!(
                "[job] MigrationImportFollows enqueue 失敗 (request_id={}): {}",
                request_id,
                e
            );
        }
    }

    /// リスト機能（#63）: list-relay 仮想アクターの代理フォロー/アンフォローを積む。
    /// 呼び出し元（`handlers::lists`）が参照カウントの0↔1遷移を判定した上で呼ぶ。
    pub async fn enqueue_proxy_follow_sync(&self, target_actor_id: i64, want_follow: bool) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::ProxyFollowSync {
                    target_actor_id,
                    want_follow,
                },
                job_priority::HIGH,
            )
            .await
        {
            tracing::error!(
                "[job] ProxyFollowSync enqueue 失敗 (target={}): {}",
                target_actor_id,
                e
            );
        }
    }

    /// 退会時、自分がフォローしていた相手（フォロイー）全員への一括アンフォロージョブを積む。
    /// 配送の実行・リトライは Worker（`jobs::account_withdraw_unfollow_all`）が担う。
    pub async fn enqueue_account_withdraw_unfollow_all(&self, actor_id: i64, username: String) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::AccountWithdrawUnfollowAll { actor_id, username },
                job_priority::HIGH,
            )
            .await
        {
            tracing::error!(
                "[job] AccountWithdrawUnfollowAll enqueue 失敗 (actor_id={}): {}",
                actor_id,
                e
            );
        }
    }

    /// フォロー承認制（鍵アカウント）をOFFに切り替えた際、その時点で存在した承認待ち
    /// フォローリクエスト全件の一括承認ジョブを積む。実行は
    /// Worker（`jobs::follow_requests_bulk_accept`）が担う。
    pub async fn enqueue_follow_requests_bulk_accept(&self, actor_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::FollowRequestsBulkAccept { actor_id },
                job_priority::HIGH,
            )
            .await
        {
            tracing::error!(
                "[job] FollowRequestsBulkAccept enqueue 失敗 (actor_id={}): {}",
                actor_id,
                e
            );
        }
    }

    /// Bsky embedとして選択された動画/音声添付のパイプライン結合完了待ちで、投稿のBsky
    /// コミットをWorker（`jobs::bsky_post_commit_deferred`）へ委譲する。`pending_media_file_id`
    /// は選択が解決した先の`media_files.id`1件のみ（#227、`resolve_bsky_embed`参照）。
    /// 本文・投稿時刻・リプライ先at_uri/at_cidはジョブのペイロードに持たせず、ハンドラが
    /// `post_id`から`posts`テーブルを都度参照する設計のため、ここでは`posts.pending_bsky_media_file_id`
    /// を先に永続化してから（起動時リカバリが検出できるようにする）enqueueする。
    pub async fn enqueue_bsky_post_commit_deferred(
        &self,
        actor_id: i64,
        post_id: i64,
        pending_media_file_id: i64,
    ) {
        if let Err(e) = seiran_common::repository::maintenance::set_pending_bsky_media_file(
            &self.db,
            post_id,
            pending_media_file_id,
        )
        .await
        {
            tracing::error!(
                "[job] pending_bsky_media_file_id 設定失敗 (post_id={}): {}",
                post_id,
                e
            );
        }

        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::BskyPostCommitDeferred {
                    actor_id,
                    post_id,
                    pending_media_file_id,
                },
                job_priority::HIGH,
            )
            .await
        {
            tracing::error!(
                "[job] BskyPostCommitDeferred enqueue 失敗 (post_id={}): {}",
                post_id,
                e
            );
        }
    }

    /// DM（`visibility='direct'`）投稿のBsky宛先への実送信（`chat.bsky.convo.sendMessage`）ジョブを積む。
    pub async fn enqueue_bsky_dm_send(&self, post_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::BskyDmSend { post_id }, job_priority::HIGH)
            .await
        {
            tracing::error!("[job] BskyDmSend enqueue 失敗 (post_id={}): {}", post_id, e);
        }
    }

    /// bsky宛DMメッセージへの絵文字リアクション付与を`chat.bsky.convo.addReaction`で
    /// Bluesky公式チャットサービスへ配送するジョブを積む。ローカルDBへの保存は
    /// 呼び出し元が既に完了済みであること。
    pub async fn enqueue_bsky_dm_reaction_add(&self, post_id: i64, actor_id: i64, content: String) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::BskyDmReactionAdd {
                    post_id,
                    actor_id,
                    content,
                },
                job_priority::HIGH,
            )
            .await
        {
            tracing::error!(
                "[job] BskyDmReactionAdd enqueue 失敗 (post_id={}): {}",
                post_id,
                e
            );
        }
    }

    /// bsky宛DMメッセージへの絵文字リアクション取消を配送するジョブを積む。
    pub async fn enqueue_bsky_dm_reaction_remove(
        &self,
        post_id: i64,
        actor_id: i64,
        content: String,
    ) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::BskyDmReactionRemove {
                    post_id,
                    actor_id,
                    content,
                },
                job_priority::HIGH,
            )
            .await
        {
            tracing::error!(
                "[job] BskyDmReactionRemove enqueue 失敗 (post_id={}): {}",
                post_id,
                e
            );
        }
    }

    /// bsky宛DMメッセージの「隠す」（`chat.bsky.convo.deleteMessageForSelf`）を配送する
    /// ジョブを積む。
    pub async fn enqueue_bsky_dm_hide(&self, post_id: i64, actor_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::BskyDmHide { post_id, actor_id }, job_priority::HIGH)
            .await
        {
            tracing::error!("[job] BskyDmHide enqueue 失敗 (post_id={}): {}", post_id, e);
        }
    }

    /// リモート Fedi アクターの followers/following 全件同期ジョブを積む（#68）。
    /// プロフィール表示時の短タイムアウト同期取得が失敗/タイムアウトした場合のフォールバック。
    ///
    /// #229: 同一 (actor_id, direction) を直近 [`REMOTE_FOLLOW_SYNC_COOLDOWN`] 以内に既に
    /// 積んでいれば再投入しない。フォロー数の多いアクターのプロフィールを何度もリロードする
    /// と、そのたびに最大5000件の`RemoteActorResolve`（優先度低）を積む重いジョブが重複投入
    /// され、同じ優先度を共有する他のジョブ（`AlsoKnownAsVerify`等）が飢餓状態になる。
    pub async fn enqueue_remote_follow_list_sync(&self, actor_id: i64, direction: String) {
        let key = (actor_id, direction.clone());
        let now = std::time::Instant::now();
        if let Some(last) = self.remote_follow_sync_recent.get(&key) {
            if now.duration_since(*last) < REMOTE_FOLLOW_SYNC_COOLDOWN {
                tracing::debug!(
                    "[job] RemoteFollowListSync enqueue 抑制（クールダウン中）: actor_id={} direction={}",
                    actor_id, direction
                );
                return;
            }
        }
        self.remote_follow_sync_recent.insert(key, now);

        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::RemoteFollowListSync {
                    actor_id,
                    direction,
                },
                job_priority::LOW,
            )
            .await
        {
            tracing::error!(
                "[job] RemoteFollowListSync enqueue 失敗 (actor_id={}): {}",
                actor_id,
                e
            );
        }
    }

    /// リモートFediアクターのfeatured collection（ピン留め投稿, #61）同期ジョブを積む。
    /// プロフィール表示のたびに呼ばれ、表示は常にDB上の既存`pinned_posts`をそのまま返す
    /// （「表示時再検証」パターン、`enqueue_also_known_as_verify`と同様）。
    pub async fn enqueue_remote_featured_sync(&self, actor_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::RemoteFeaturedSync { actor_id }, job_priority::LOW)
            .await
        {
            tracing::error!(
                "[job] RemoteFeaturedSync enqueue 失敗 (actor_id={}): {}",
                actor_id,
                e
            );
        }
    }

    /// リモートアクターのプロフィール（avatar_url/banner_url等）再取得ジョブを積む。
    /// プロフィール表示のたびに呼ばれ、表示は常にDB上の既存値をそのまま返す
    /// （「表示時再検証」パターン、`enqueue_remote_featured_sync`と同様）。
    pub async fn enqueue_remote_profile_refresh(&self, actor_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::RemoteProfileRefresh { actor_id }, job_priority::LOW)
            .await
        {
            tracing::error!(
                "[job] RemoteProfileRefresh enqueue 失敗 (actor_id={}): {}",
                actor_id,
                e
            );
        }
    }

    /// brid.gyブリッジユーザーの実ユーザーへのリンク解決ジョブを積む。プロフィール表示の
    /// たびに、`bridge_real_actor_id`が未解決なブリッジユーザーに対して呼ばれる
    /// （「表示時再検証」パターン、`docs/protocols.md`参照）。
    pub async fn enqueue_bridge_user_link_resolve(&self, actor_id: i64) {
        if !seiran_common::jobs::bridge_user_link_resolve::should_enqueue(actor_id) {
            return;
        }
        if let Err(e) = self
            .job_queue
            .enqueue(Job::BridgeUserLinkResolve { actor_id }, job_priority::LOW)
            .await
        {
            tracing::error!(
                "[job] BridgeUserLinkResolve enqueue 失敗 (actor_id={}): {}",
                actor_id,
                e
            );
        }
    }

    /// リモートフォロー一覧中の未知アクター（ローカルDB未登録）を解決するジョブを積む（#68）。
    pub async fn enqueue_remote_actor_resolve(&self, uri: String) {
        if !seiran_common::jobs::remote_actor_resolve::should_enqueue(&uri) {
            tracing::debug!(
                "[job] RemoteActorResolve enqueue 抑制（クールダウン中）: uri={}",
                uri
            );
            return;
        }

        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::RemoteActorResolve { uri: uri.clone() },
                job_priority::LOW,
            )
            .await
        {
            tracing::error!("[job] RemoteActorResolve enqueue 失敗 (uri={}): {}", uri, e);
        }
    }

    /// bio/profile_fields中の未解決URLをFedi/Bskyのアクター・投稿として非同期解決する
    /// ジョブを積む（#リンク解決）。プロフィール取得時、未キャッシュまたはTTL経過済み
    /// 陰性URLに対して呼ばれる。
    pub async fn enqueue_link_resolve(&self, url: String) {
        if !seiran_common::jobs::link_resolve::should_enqueue(&url) {
            tracing::debug!(
                "[job] LinkResolve enqueue 抑制（クールダウン中）: url={}",
                url
            );
            return;
        }

        if let Err(e) = self
            .job_queue
            .enqueue(Job::LinkResolve { url: url.clone() }, job_priority::LOW)
            .await
        {
            tracing::error!("[job] LinkResolve enqueue 失敗 (url={}): {}", url, e);
        }
    }

    /// プロフィールの「別のアカウント」相互検証ジョブを積む。プロフィール表示のたびに
    /// 呼ばれ、表示は常にキャッシュ済みの検証結果を読むだけで、この結果は次回表示時に
    /// 反映される（「表示時再検証」パターン、`docs/architecture.md`参照）。
    pub async fn enqueue_also_known_as_verify(&self, owner_actor_id: i64, target_actor_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::AlsoKnownAsVerify {
                    owner_actor_id,
                    target_actor_id,
                },
                job_priority::LOW,
            )
            .await
        {
            tracing::error!(
                "[job] AlsoKnownAsVerify enqueue 失敗 (owner={}, target={}): {}",
                owner_actor_id,
                target_actor_id,
                e
            );
        }
    }

    /// プロフィールの「別のアカウント」表示: リモートFediアクター自身のalsoKnownAs自己申告を
    /// 取り込む同期ジョブを積む。
    pub async fn enqueue_remote_also_known_as_sync(&self, owner_actor_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::RemoteAlsoKnownAsSync { owner_actor_id },
                job_priority::LOW,
            )
            .await
        {
            tracing::error!(
                "[job] RemoteAlsoKnownAsSync enqueue 失敗 (owner={}): {}",
                owner_actor_id,
                e
            );
        }
    }

    /// リモートインスタンスのnodeinfo取得ジョブを積む（#NoteCardリモートサーバー表示）。
    /// `remote_instance_meta` に未登録のドメインを見つけた際、表示のリッチ化目的で積む。
    pub async fn enqueue_remote_instance_info_resolve(&self, domain: String) {
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::RemoteInstanceInfoResolve {
                    domain: domain.clone(),
                },
                job_priority::LOW,
            )
            .await
        {
            tracing::error!(
                "[job] RemoteInstanceInfoResolve enqueue 失敗 (domain={}): {}",
                domain,
                e
            );
        }
    }

    /// リモートアンケートの生存監視フォールバック取得ジョブを積む（Update(Question)を
    /// 送ってこない実装への保険、`handlers::notes::queries::enqueue_stale_poll_fetches`が使う）。
    pub async fn enqueue_poll_fetch(&self, post_id: i64) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::PollFetch { post_id }, job_priority::LOW)
            .await
        {
            tracing::error!("[job] PollFetch enqueue 失敗 (post_id={}): {}", post_id, e);
        }
    }
}

/// 共有リソース（DB プール・シークレット・HTTP クライアント・ドメイン）を受け取り
/// api ロールの [`AppState`] を構築する。
///
/// `seiran-server` が単一プロセス内でこれらのリソースを一度だけ生成し、
/// 各ロールの `init_state` へ渡す（`all` モードでの重複接続を避けるため）。
pub async fn init_state(
    pool: PgPool,
    secrets: Arc<Secrets>,
    http_client: Arc<reqwest::Client>,
    local_domain: seiran_common::LocalDomain,
    job_queue: Arc<dyn JobQueue>,
    // `Some` なら ATP コミットイベントを Redis Pub/Sub 経由でプロセス間配信する
    // ブリッジを有効にする（`api` ロールを複数レプリカで水平スケールする場合に必要。
    // モノリスモードや単一レプリカ運用では `None` でよい）。
    atp_event_redis_url: Option<String>,
) -> AppState {
    let local_auth = Arc::new(LocalAuthProvider::new(secrets.jwt_secret_bytes()));
    let ap_client = Arc::new(ApClient::new(Arc::clone(&http_client)));

    let (atp_event_tx, _) = broadcast::channel::<AtpCommitEvent>(1024);
    let atp_event_tx = Arc::new(atp_event_tx);

    let mut atp_service = AtpCommitService::new(
        pool.clone(),
        Arc::clone(&atp_event_tx),
        Arc::clone(&http_client),
        local_domain.clone(),
    );
    if let Some(redis_url) = atp_event_redis_url {
        match atp_service.with_redis_bridge(&redis_url).await {
            Ok(()) => tracing::info!("[seiran-api] ATPコミットイベント: Redisプロセス間配信ブリッジ有効"),
            Err(e) => tracing::error!(
                "[seiran-api] ATPコミットイベントのRedisブリッジ有効化に失敗（プロセス内配信のみで続行）: {}",
                e
            ),
        }
    }
    let atp_service = Arc::new(atp_service);

    let cloudflare = match (
        std::env::var("CLOUDFLARE_API_TOKEN"),
        std::env::var("CLOUDFLARE_ZONE_ID"),
    ) {
        (Ok(token), Ok(zone_id)) if !token.is_empty() && !zone_id.is_empty() => {
            tracing::info!("[seiran-api] Cloudflare DNS ハンドル検証: 有効");
            Some(Arc::new(cloudflare::CloudflareClient::new(
                Arc::clone(&http_client),
                token,
                zone_id,
            )))
        }
        _ => {
            tracing::info!("[seiran-api] Cloudflare DNS ハンドル検証: 無効 (HTTP well-known のみ)");
            None
        }
    };

    let enc_key = secrets.encryption_key_bytes();
    let storage_providers: Arc<dyn StorageProviderRepository> =
        Arc::new(PgStorageProviderRepository::new(pool.clone(), enc_key));
    let media_files: Arc<dyn MediaFileRepository> =
        Arc::new(PgMediaFileRepository::new(pool.clone()));
    let site_settings: Arc<dyn SiteSettingsRepository> =
        Arc::new(PgSiteSettingsRepository::new(pool.clone()));
    let oembed_whitelist = Arc::new(seiran_common::oembed_whitelist::OembedWhitelist::new(
        site_settings.clone(),
    ));
    let instance_domain: Arc<dyn InstanceDomainRepository> =
        Arc::new(PgInstanceDomainRepository::new(pool.clone()));
    let remote_instance_meta: Arc<dyn RemoteInstanceMetaRepository> =
        Arc::new(PgRemoteInstanceMetaRepository::new(pool.clone()));
    let link_resolutions: Arc<dyn LinkResolutionRepository> =
        Arc::new(PgLinkResolutionRepository::new(pool.clone()));
    let actors: Arc<dyn ActorRepository> = Arc::new(PgActorRepository::new(pool.clone()));
    let users: Arc<dyn UserRepository> = Arc::new(PgUserRepository::new(pool.clone()));
    let posts: Arc<dyn PostRepository> = Arc::new(PgPostRepository::new(pool.clone()));
    let follows: Arc<dyn FollowRepository> = Arc::new(PgFollowRepository::new(pool.clone()));
    let follow_imports: Arc<dyn FollowImportRepository> =
        Arc::new(PgFollowImportRepository::new(pool.clone()));
    let blocks: Arc<dyn BlockRepository> = Arc::new(PgBlockRepository::new(pool.clone()));
    let mutes: Arc<dyn MuteRepository> = Arc::new(PgMuteRepository::new(pool.clone()));
    let repost_mutes: Arc<dyn RepostMuteRepository> =
        Arc::new(PgRepostMuteRepository::new(pool.clone()));
    let app_tokens: Arc<dyn AppTokenRepository> = Arc::new(PgAppTokenRepository::new(pool.clone()));
    let atp_repo: Arc<dyn AtpReadRepository> = Arc::new(PgAtpReadRepository::new(pool.clone()));
    let atp_sessions: Arc<dyn AtpSessionRepository> =
        Arc::new(PgAtpSessionRepository::new(pool.clone()));
    let atp_preferences: Arc<dyn AtpPreferencesRepository> =
        Arc::new(PgAtpPreferencesRepository::new(pool.clone()));
    let reactions: Arc<dyn ReactionRepository> = Arc::new(PgReactionRepository::new(pool.clone()));
    let pinned_posts: Arc<dyn PinnedPostsRepository> =
        Arc::new(PgPinnedPostsRepository::new(pool.clone()));
    let notifications: Arc<dyn NotificationRepository> =
        Arc::new(PgNotificationRepository::new(pool.clone()));
    let dm: Arc<dyn DmRepository> = Arc::new(PgDmRepository::new(pool.clone()));
    let lists: Arc<dyn ListRepository> = Arc::new(PgListRepository::new(pool.clone()));
    let also_known_as: Arc<dyn AlsoKnownAsRepository> =
        Arc::new(PgAlsoKnownAsRepository::new(pool.clone()));
    let hashtags: Arc<dyn HashtagRepository> = Arc::new(PgHashtagRepository::new(pool.clone()));
    let password_resets: Arc<dyn PasswordResetRepository> =
        Arc::new(PgPasswordResetRepository::new(pool.clone()));
    let auth_rate_limits: Arc<dyn AuthRateLimitRepository> =
        Arc::new(PgAuthRateLimitRepository::new(pool.clone()));
    let email_verifications: Arc<dyn EmailVerificationRepository> =
        Arc::new(PgEmailVerificationRepository::new(pool.clone()));
    let email_changes: Arc<dyn EmailChangeRepository> =
        Arc::new(PgEmailChangeRepository::new(pool.clone()));
    let email_short_codes: Arc<dyn EmailShortCodeRepository> =
        Arc::new(PgEmailShortCodeRepository::new(pool.clone()));
    let emojis: Arc<dyn EmojiRepository> = Arc::new(PgEmojiRepository::new(pool.clone()));
    let remote_emojis: Arc<dyn RemoteEmojiRepository> =
        Arc::new(PgRemoteEmojiRepository::new(pool.clone()));
    let relays: Arc<dyn RelayRepository> = Arc::new(PgRelayRepository::new(pool.clone()));
    let totp: Arc<dyn TotpRepository> = Arc::new(PgTotpRepository::new(pool.clone()));

    let system_proxy_actor_id =
        match seiran_common::ensure_system_proxy_actor(&pool, &local_domain).await {
            Ok(id) => id,
            Err(e) => {
                // 起動を止めるほどの障害ではない（リスト機能のプロキシフォローが動かないだけ）ため、
                // ログのみに留めて 0（実在しない actor_id）で継続する。
                tracing::error!(
                    "[seiran-api] list-relay プロキシアクターの準備に失敗: {}",
                    e
                );
                0
            }
        };

    if let Err(e) = seiran_common::ensure_relay_agent_actor(&pool, &local_domain).await {
        tracing::error!("[seiran-api] relay-agent アクターの準備に失敗: {}", e);
    }

    let rp_origin_value =
        std::env::var("WEBAUTHN_ORIGIN").unwrap_or_else(|_| format!("https://{}", local_domain));
    let rp_origin =
        Url::parse(&rp_origin_value).expect("LOCAL_DOMAINからWebAuthn originを構築できません");
    let webauthn = Arc::new(
        WebauthnBuilder::new(&local_domain, &rp_origin)
            .expect("WebAuthn relying party設定が不正です")
            .rp_name("seiran")
            .build()
            .expect("WebAuthn初期化に失敗しました"),
    );

    AppState {
        actors,
        also_known_as,
        users,
        posts,
        follows,
        follow_imports,
        blocks,
        mutes,
        repost_mutes,
        app_tokens,
        atp_repo,
        atp_sessions,
        atp_preferences,
        reactions,
        pinned_posts,
        notifications,
        dm,
        db: pool,
        local_auth,
        miauth_sessions: Arc::new(RwLock::new(HashMap::new())),
        local_domain,
        instance_domain,
        remote_instance_meta,
        link_resolutions,
        frontend_origin: std::env::var("FRONTEND_ORIGIN")
            .unwrap_or_else(|_| "http://frontend:5173".to_string()),
        secrets,
        atp_service,
        http_client,
        ap_client,
        cloudflare,
        storage_providers,
        media_files,
        site_settings,
        oembed_whitelist,
        search_store: Arc::new(InMemorySearchStore::new()),
        stream_hub: Arc::new(StreamHub::new()),
        emoji_import_jobs: Arc::new(DashMap::new()),
        remote_follow_sync_recent: Arc::new(DashMap::new()),
        job_queue,
        lists,
        hashtags,
        system_proxy_actor_id,
        password_resets,
        auth_rate_limits,
        email_verifications,
        email_changes,
        email_short_codes,
        emojis,
        remote_emojis,
        relays,
        totp,
        webauthn,
    }
}

/// 起動時タスク: 全ローカルユーザーの Cloudflare TXT 再登録 → Relay requestCrawl →
/// #identity イベントのバックフィル、をこの順でバックグラウンド実行する。
pub fn spawn_startup_tasks(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        resume_running_follow_imports(&state).await;
        resume_running_migrations(&state).await;
        resume_account_withdraw_unfollow_all(&state).await;
        resume_bsky_video_poll(&state).await;
        resume_bsky_post_commit_deferred(&state).await;
        ensure_handle_txt_records(&state).await;
        request_relay_crawl(&state).await;
        // requestCrawl 後、Relay が subscribeRepos に接続するまで待機してから
        // #identity をブロードキャストする。
        tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
        backfill_identity_events(&state).await;
        backfill_unset_avatar_profiles(&state).await;
        backfill_chat_declarations(&state).await;
        backfill_seiran_actor_declarations(&state).await;
        backfill_remote_instance_meta(&state).await;
    });
}

/// 起動時リカバリ: プロセス再起動で停止したフォローインポートのジョブチェーンを再開する。
/// `Job::FollowImportProcess` の遅延リトライ（レート制限待ち）はInMemoryJobQueueでは
/// プロセス内メモリのみで管理されており、プロセス再起動で消失するため、`running` 状態の
/// リクエストは自然には再開しない。ここで無条件に全件再enqueueする（「最後の進捗から
/// 一定時間経過したものだけ」のように絞り込むと、絞り込み条件の見積もり次第で
/// 本当に停止しているチェーンを見逃す投入漏れの方が実害として大きいため、あえて絞らない）。
/// 重複投入（正常に動いているチェーンへの余分な再enqueue）は
/// `jobs::follow_import` の `request_id` 単位 advisory lock が自然に解消する。
async fn resume_running_follow_imports(state: &AppState) {
    let request_ids: Vec<i64> =
        match seiran_common::repository::maintenance::running_follow_import_ids(&state.db).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!("[startup] 実行中フォローインポートの取得失敗: {}", e);
                return;
            }
        };
    if request_ids.is_empty() {
        return;
    }
    tracing::info!(
        "[startup] 実行中フォローインポート {} 件を再開します",
        request_ids.len()
    );
    for request_id in request_ids {
        state.enqueue_follow_import_process(request_id).await;
    }
}

/// 起動時リカバリ: プロセス再起動で停止した既存DID転入フロー（`docs/account_migration.md`）の
/// ジョブチェーンを再開する。`resume_running_follow_imports`と同じ理由（InMemoryJobQueueの
/// 遅延リトライがプロセス内メモリのみで消失しうる）で、対象ステータスの行を無条件に
/// 再enqueueする。重複投入は各ジョブの`request_id`単位advisory lockが解消する。
///
/// ステータスごとに対応するジョブが異なるため、状態→enqueue関数の対応表として管理する。
/// Phase 5でインポート系ジョブが増えたらここに追加する。`awaiting_*`（ユーザー入力待ち）と
/// `submitting_plc`（HTTPハンドラの再入で完結、ジョブ化していない）は対象外。
async fn resume_running_migrations(state: &AppState) {
    use seiran_common::repository::{AtMigrationRepository, PgAtMigrationRepository};

    let repo = PgAtMigrationRepository::new(state.db.clone());

    for (status, label) in [
        ("fetching_repo", "getRepo取得"),
        ("requesting_plc_signature", "PLC署名リクエスト"),
        ("importing_data", "データ取り込み"),
        ("deactivating_source", "移行元アカウント無効化"),
    ] {
        let request_ids = match repo.list_by_statuses(&[status]).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!(
                    "[startup] 実行中の既存DID転入リクエスト取得失敗 (status={}): {}",
                    status,
                    e
                );
                continue;
            }
        };
        if request_ids.is_empty() {
            continue;
        }
        tracing::info!(
            "[startup] 実行中の既存DID転入リクエスト（{}）{} 件を再開します",
            label,
            request_ids.len()
        );
        for request_id in request_ids {
            match status {
                "fetching_repo" => state.enqueue_migration_fetch_repo(request_id).await,
                "requesting_plc_signature" => {
                    state
                        .enqueue_migration_request_plc_signature(request_id)
                        .await
                }
                "importing_data" => state.enqueue_migration_import_process(request_id).await,
                "deactivating_source" => {
                    state.enqueue_migration_deactivate_source(request_id).await
                }
                _ => unreachable!(),
            }
        }
    }

    // フォロー関係の復元（`Job::MigrationImportFollows`）は`status`とは独立した
    // 結果整合処理のため、上のstatus起点ループとは別に判定する。
    match repo
        .list_request_ids_with_pending_follow_materialization()
        .await
    {
        Ok(request_ids) if !request_ids.is_empty() => {
            tracing::info!(
                "[startup] フォロー関係復元待ちの既存DID転入リクエスト {} 件を再開します",
                request_ids.len()
            );
            for request_id in request_ids {
                state.enqueue_migration_import_follows(request_id).await;
            }
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!("[startup] フォロー関係復元待ちリクエスト取得失敗: {}", e);
        }
    }
}

/// 起動時リカバリ: プロセス再起動で停止した退会時一括アンフォロー（`Job::AccountWithdrawUnfollowAll`）
/// を再開する。`actors.withdrawn_at` が設定済み（退会済み）なのに `follows` にまだ
/// フォロー先が残っているアクターを検出し、無条件で全件再enqueueする（`resume_running_follow_imports`
/// と同じ理由で絞り込まない）。重複投入は`jobs::account_withdraw_unfollow_all`の
/// `actor_id` 単位 advisory lock が解消する。
async fn resume_account_withdraw_unfollow_all(state: &AppState) {
    let rows = match seiran_common::repository::maintenance::withdrawn_actors_with_follows(
        &state.db,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[startup] 退会済みアクターの残存フォロー確認失敗: {}", e);
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    tracing::info!(
        "[startup] 退会済みアクターの一括アンフォロー未完了 {} 件を再開します",
        rows.len()
    );
    for (actor_id, username) in rows {
        state
            .enqueue_account_withdraw_unfollow_all(actor_id, username)
            .await;
    }
}

/// 起動時リカバリ: プロセス再起動で停止したBsky動画パイプライン結合待ち（`Job::BskyVideoPoll`）
/// を再開する。`media_files.bsky_video_status = 'pending'`（`app.bsky.video.uploadVideo`へ
/// 提出済みだが `ready`/`failed` に未確定）を無条件で全件再enqueueする。重複投入は
/// `jobs::bsky_video_poll`の `media_file_id` 単位 advisory lock が解消する。
async fn resume_bsky_video_poll(state: &AppState) {
    let media_file_ids: Vec<i64> =
        match seiran_common::repository::maintenance::pending_bsky_video_media_ids(&state.db).await
        {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!("[startup] Bsky動画パイプライン結合待ちの確認失敗: {}", e);
                return;
            }
        };
    if media_file_ids.is_empty() {
        return;
    }
    tracing::info!(
        "[startup] Bsky動画パイプライン結合待ち {} 件を再開します",
        media_file_ids.len()
    );
    for media_file_id in media_file_ids {
        if let Err(e) = state
            .job_queue
            .enqueue(Job::BskyVideoPoll { media_file_id }, job_priority::HIGH)
            .await
        {
            tracing::error!(
                "[startup] BskyVideoPoll enqueue 失敗 (media_file_id={}): {}",
                media_file_id,
                e
            );
        }
    }
}

/// 起動時リカバリ: プロセス再起動で停止した動画添付投稿のBskyコミット遅延
/// （`Job::BskyPostCommitDeferred`）を再開する。`posts.pending_bsky_media_file_id`が
/// 設定済み（`enqueue_bsky_post_commit_deferred`が投稿作成時点で永続化した値）かつ
/// `at_uri`が未確定（まだBskyへコミットされていない）投稿を無条件で全件再enqueueする
/// （`resume_running_follow_imports`と同じ理由で絞り込まない）。重複投入は
/// `jobs::bsky_post_commit_deferred`の`post_id`単位advisory lockが解消する。
async fn resume_bsky_post_commit_deferred(state: &AppState) {
    let rows =
        match seiran_common::repository::maintenance::pending_bsky_post_commits(&state.db).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("[startup] Bskyコミット遅延未完了の確認失敗: {}", e);
                return;
            }
        };
    if rows.is_empty() {
        return;
    }
    tracing::info!(
        "[startup] Bskyコミット遅延未完了 {} 件を再開します",
        rows.len()
    );
    for (post_id, actor_id, pending_media_file_id) in rows {
        state
            .enqueue_bsky_post_commit_deferred(actor_id, post_id, pending_media_file_id)
            .await;
    }
}

/// 既存の全リモートFedi/seiran間連合ドメインのうち`remote_instance_meta`未登録のものを
/// まとめて`RemoteInstanceInfoResolve`ジョブへ積む（#NoteCardリモートサーバー表示）。
/// 通常はnotes API呼び出し時の遅延解決（`queries::attach_remote_instance_info`）で
/// 徐々に埋まっていくが、起動時にこれを走らせることで新規デプロイ直後の
/// 大量未解決状態（既存ドメイン全件が対象）を素早く解消する。
/// `icon_url`/`node_name`/`software_name`がNULLの行も対象に含める: 取得項目を後から
/// 足したとき、それ以前に解決済みの行が`NOT EXISTS`だけの判定だと永久に再取得されない。
/// `software_name IS NULL`も同じ扱いにしているのは、nodeinfoドキュメントの一時的な
/// パース失敗・discovery失敗（`jobs::remote_instance_info_resolve`の「諦め」分岐）で
/// 一度NULLキャッシュされると、当時は本当に非対応だったとしても後日そのソフトウェア側で
/// nodeinfo対応が追加・修正される場合があり、`software_name`が埋まらない限り固有色
/// フォールバックも一生適用されないため。
/// 非対応サーバーは毎回再チャレンジすることになるが、起動時のみの発生でありコストは小さい。
///
/// `theme_color`が汎用デフォルト（`DEFAULT_THEME_COLOR`）のまま止まっている行のうち、
/// `fallback_color_for_software`（既知フォーク固有色表）に現在その`software_name`が
/// 載っているものも対象に含める: `themeColor`未宣言サーバー向けの固有色を後から追加した
/// 際、それ以前に解決済みだった行が汎用グレーのまま固定され、再解決の手段が
/// `NOT EXISTS`判定に無いため永久に放置されるのを防ぐ。固有色未登録のsoftware（意図的に汎用グレーへフォールバックした行）は
/// 対象外なので、毎起動で無限に再チャレンジすることはない。
async fn backfill_remote_instance_meta(state: &AppState) {
    let domains = match seiran_common::repository::maintenance::domains_missing_instance_meta(
        &state.db,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[startup] remote_instance_meta backfill対象取得失敗: {}", e);
            return;
        }
    };

    let stale_color_rows =
        match seiran_common::repository::maintenance::instance_meta_with_theme_color(
            &state.db,
            seiran_common::jobs::remote_instance_info_resolve::DEFAULT_THEME_COLOR,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(
                    "[startup] remote_instance_meta 固有色backfill対象取得失敗: {}",
                    e
                );
                Vec::new()
            }
        };

    let mut targets: Vec<String> = domains;
    for (domain, software_name) in stale_color_rows {
        let has_fallback = software_name
            .as_deref()
            .and_then(
                seiran_common::jobs::remote_instance_info_resolve::fallback_color_for_software,
            )
            .is_some();
        if has_fallback {
            targets.push(domain);
        }
    }
    targets.sort();
    targets.dedup();

    let total = targets.len();
    for domain in targets {
        state.enqueue_remote_instance_info_resolve(domain).await;
    }
    tracing::info!(
        "[startup] remote_instance_meta backfill: {}件のドメインを解決ジョブへ積みました",
        total
    );
}

/// 明示的に有効化した起動時だけ、アバター未設定ユーザーのプロフィールを再コミットする。
/// 既存レコードから新しい #commit を生成し、Relay/AppView にプロフィール再取得を促す。
async fn backfill_unset_avatar_profiles(state: &AppState) {
    if std::env::var("ATP_BACKFILL_UNSET_AVATAR_PROFILES_ONCE").as_deref() != Ok("1") {
        return;
    }

    let actor_ids =
        match seiran_common::repository::maintenance::local_actor_ids_without_avatar(&state.db)
            .await
        {
            Ok(ids) => ids,
            Err(error) => {
                tracing::error!(
                    "[startup] 未設定アバタープロフィール対象取得失敗: {}",
                    error
                );
                return;
            }
        };

    let total = actor_ids.len();
    let mut succeeded = 0usize;
    for actor_id in actor_ids {
        let material = handlers::notes::fetch_atp_profile_material(state, actor_id).await;
        let pinned_post = handlers::notes::resolve_bsky_pinned_post(state, actor_id).await;
        match material {
            Ok((display_name, description, avatar_media, banner_media)) => match state
                .atp_service
                .commit_profile(
                    actor_id,
                    &seiran_common::atp::service::ProfileCommit {
                        display_name: &display_name,
                        description: description.as_deref(),
                        avatar_media,
                        banner_media,
                        pinned_post,
                    },
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(()) => succeeded += 1,
                Err(error) => tracing::error!(
                    "[startup] actor_id={} の未設定アバタープロフィール再コミット失敗: {}",
                    actor_id,
                    error
                ),
            },
            Err(error) => tracing::error!(
                "[startup] actor_id={} のATPプロフィール材料取得失敗: {}",
                actor_id,
                error
            ),
        }
    }
    tracing::info!(
        "[startup] 未設定アバタープロフィール再コミット完了: {}/{}",
        succeeded,
        total
    );
}

/// 全ローカルユーザーの ATP ハンドル TXT レコードを確保する（再デプロイ後の消失対策）。
async fn ensure_handle_txt_records(state: &AppState) {
    let Some(cf) = state.cloudflare.as_ref() else {
        return;
    };
    let rows = match state.actors.list_local_dids().await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[startup] ローカルユーザー取得失敗: {}", e);
            return;
        }
    };
    for (username, did) in rows {
        let handle = format!(
            "{}.{}",
            seiran_common::username::to_atp_username(&username),
            state.local_domain
        );
        match cf.ensure_atproto_txt(&handle, &did).await {
            Ok(_) => tracing::info!("[startup] TXT 確認済み: _atproto.{}", handle),
            Err(e) => tracing::error!("[startup] TXT 登録失敗: {}: {}", handle, e),
        }
    }
}

/// Relay に requestCrawl を送って subscribeRepos 再接続を促す。
/// ATP_RELAY_URL はカンマ区切りで複数指定でき、全てへ並行して送る
/// （AtpCommitService::spawn_request_crawl と同じ規約）。
async fn request_relay_crawl(state: &AppState) {
    let relay_base_raw =
        std::env::var("ATP_RELAY_URL").unwrap_or_else(|_| "https://bsky.network".to_string());
    let relay_bases: Vec<String> = relay_base_raw
        .split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for relay_base in relay_bases {
        let url = format!("{}/xrpc/com.atproto.sync.requestCrawl", relay_base);
        match state
            .http_client
            .post(&url)
            .json(&serde_json::json!({"hostname": state.local_domain.as_str()}))
            .send()
            .await
        {
            Ok(res) => tracing::info!("[atp] 起動時 requestCrawl({}) → {}", url, res.status()),
            Err(e) => tracing::error!("[atp] 起動時 requestCrawl({}) 失敗: {}", url, e),
        }
    }
}

/// #identity イベントが未送出の既存ローカルユーザー分を DB 保存 + broadcast する。
async fn backfill_identity_events(state: &AppState) {
    let now = chrono::Utc::now();
    let missing = match seiran_common::repository::maintenance::local_actors_without_identity_event(
        &state.db,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[startup] #identity 対象取得失敗: {}", e);
            return;
        }
    };

    for (actor_id, username, did) in missing {
        let handle = format!(
            "{}.{}",
            seiran_common::username::to_atp_username(&username),
            state.local_domain
        );
        match state
            .atp_service
            .broadcast_identity_event(actor_id, &did, &handle, now)
            .await
        {
            Ok(_) => tracing::info!("[startup] #identity broadcast: {}", handle),
            Err(e) => tracing::error!("[startup] #identity 失敗 {}: {}", handle, e),
        }
    }
}

/// 既存ユーザー（DM機能実装前に登録済み）向けに `chat.bsky.actor.declaration` を
/// バックフィルする。このレコードが無いとBluesky公式クライアントは相手（seiranユーザー）
/// へのDM送信を保守的にブロックする（`docs/protocols.md` 9節）。
async fn backfill_chat_declarations(state: &AppState) {
    let now = chrono::Utc::now();
    let missing: Vec<i64> =
        match seiran_common::repository::maintenance::local_actors_without_self_record(
            &state.db,
            "chat.bsky.actor.declaration",
        )
        .await
        .map(|rows| rows.into_iter().map(|(id, _)| id).collect::<Vec<_>>())
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[startup] chat declaration 対象取得失敗: {}", e);
                return;
            }
        };

    for actor_id in missing {
        match state
            .atp_service
            .commit_chat_declaration(actor_id, now)
            .await
        {
            Ok(_) => tracing::info!("[startup] chat declaration commit: actor_id={}", actor_id),
            Err(e) => tracing::error!(
                "[startup] chat declaration 失敗 actor_id={}: {}",
                actor_id,
                e
            ),
        }
    }
}

/// リモートseiranアクターの相互申告マージ（#236）用の自己申告を、まだ未コミットの
/// ローカルユーザーへ一括バックフィルする（`backfill_chat_declarations`と同じパターン）。
async fn backfill_seiran_actor_declarations(state: &AppState) {
    let now = chrono::Utc::now();
    let missing = match seiran_common::repository::maintenance::local_actors_without_self_record(
        &state.db,
        "org.seiran.actor.declaration",
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[startup] seiran actor declaration 対象取得失敗: {}", e);
            return;
        }
    };

    for (actor_id, username) in missing {
        let ap_actor_uri = format!("https://{}/users/{}", state.local_domain, username);
        match state
            .atp_service
            .commit_seiran_actor_declaration(actor_id, &ap_actor_uri, now)
            .await
        {
            Ok(_) => tracing::info!(
                "[startup] seiran actor declaration commit: actor_id={}",
                actor_id
            ),
            Err(e) => tracing::error!(
                "[startup] seiran actor declaration 失敗 actor_id={}: {}",
                actor_id,
                e
            ),
        }
    }
}

// =====================================================================
// メディア GC タスク
// =====================================================================

/// アップロードされたが参照されていない media_files を定期的に削除するタスク。
///
/// 1時間ごとに孤立ファイル（最終アップロードから7日以上経過かつどのテーブルからも
/// 参照なし）を S3 → DB の順でベストエフォートで削除する。同じ周期で
/// atp_repo_events.car_bytes の定期NULL化（PERF-5）も行う。
pub fn spawn_gc_tasks(state: &AppState) {
    // 検索セッション GC（1分ごとにタイムアウトしたセッションを削除）
    let search_store = Arc::clone(&state.search_store);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            search_store.cleanup();
        }
    });

    let db = state.db.clone();
    let storage_providers = Arc::clone(&state.storage_providers);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            run_media_gc(&db, storage_providers.as_ref()).await;
        }
    });

    // [PERF-5] atp_repo_events.car_bytes（subscribeReposバックフィル用の差分CAR）の定期NULL化。
    // Relay側が過去イベントの再取得を要求してくるのは実運用上せいぜい数十時間程度で、
    // それを過ぎたら car_bytes を保持し続ける理由がない（イベントの行自体・seq・
    // ops_jsonは残し、容量の大半を占めるバイト列のみ落とす）。
    let db3 = state.db.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            run_atp_repo_events_car_bytes_gc(&db3).await;
        }
    });
}

/// 孤立ファイルを最大 100 件取得し、DB → S3 の順で削除する（ベストエフォート）。
///
/// 候補取得（SELECT）から実際の削除までの間に別のリクエストがそのファイルを新たに
/// 参照し始める競合を防ぐため、DB側の削除は「削除する瞬間に孤立条件を再評価する」
/// 単一の `DELETE ... WHERE 孤立条件 RETURNING` 文で行う（PostgreSQLの単一ステートメント
/// はアトミックなので、候補取得時点のスナップショットに基づくTOCTOUが起こらない）。
///
/// DB削除が確定してからS3の実体を消す（逆順ではない）。これにより「media_filesに行が
/// あるなら対応するS3オブジェクトも必ずある」という逆方向の不変条件になり、S3削除が
/// 失敗してもDB行が既に消えている分にはAPI利用者から見た整合性は壊れない（S3側にだけ
/// ゴミが残るが、これは検出用の別パトロールで回収すればよく実害が小さい）。
async fn run_media_gc(pool: &sqlx::PgPool, storage_providers: &dyn StorageProviderRepository) {
    let rows = match seiran_common::repository::maintenance::orphaned_media_files(pool, 100).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[media-gc] 孤立ファイル取得失敗: {}", e);
            return;
        }
    };

    if rows.is_empty() {
        return;
    }
    tracing::info!("[media-gc] 孤立ファイル {} 件を処理します", rows.len());

    for row in rows {
        let provider = match storage_providers.find_by_id(row.storage_provider_id).await {
            Ok(Some(provider)) => provider,
            Ok(None) => {
                tracing::warn!(
                    "[media-gc] プロバイダー不明 id={}, provider_id={}",
                    row.id,
                    row.storage_provider_id
                );
                continue;
            }
            Err(e) => {
                tracing::error!("[media-gc] プロバイダー取得失敗: {}", e);
                continue;
            }
        };

        let deleted = match seiran_common::repository::maintenance::delete_media_file_if_orphaned(
            pool, row.id,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[media-gc] DB 削除失敗 id={}: {}", row.id, e);
                continue;
            }
        };
        if !deleted {
            tracing::info!(
                "[media-gc] id={} は削除直前に参照が追加されたためスキップ",
                row.id
            );
            continue;
        }

        let s3 = S3StorageClient::new(&provider);
        if let Err(e) = s3.delete(&row.storage_key).await {
            tracing::error!(
                "[media-gc] S3 削除失敗（DB行は削除済み）id={}: {}",
                row.id,
                e
            );
        } else {
            tracing::info!("[media-gc] 削除完了 id={}", row.id);
        }
    }
}

/// [PERF-5] 72時間以上経過した `atp_repo_events.car_bytes` を NULL 化する
/// （イベント行・`ops_json`は残し、容量の大半を占めるバイト列のみ落とす）。
async fn run_atp_repo_events_car_bytes_gc(pool: &sqlx::PgPool) {
    match seiran_common::repository::maintenance::drop_old_repo_event_car_bytes(pool).await {
        Ok(n) if n > 0 => {
            tracing::info!("[atp-repo-events-gc] car_bytes を {} 件NULL化しました", n);
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!("[atp-repo-events-gc] car_bytes NULL化失敗: {}", e);
        }
    }
}
