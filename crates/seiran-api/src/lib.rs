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
pub mod search;
pub mod search_query;
pub mod streaming;

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post},
    Router,
};
use dashmap::DashMap;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::{Any, CorsLayer};
use webauthn_rs::prelude::{Url, Webauthn, WebauthnBuilder};

use seiran_common::repository::{
    ActorRepository, AlsoKnownAsRepository, AppTokenRepository, AtpPreferencesRepository,
    AtpReadRepository, AtpSessionRepository, AuthRateLimitRepository, BlockRepository,
    DmRepository, EmailChangeRepository, EmailVerificationRepository, EmojiRepository,
    FollowImportRepository, FollowRepository, HashtagRepository, InstanceDomainRepository,
    ListRepository, MuteRepository, NotificationRepository, PasswordResetRepository,
    PgActorRepository, PgAlsoKnownAsRepository, PgAppTokenRepository, PgAtpPreferencesRepository,
    PgAtpReadRepository, PgAtpSessionRepository, PgAuthRateLimitRepository, PgBlockRepository,
    PgDmRepository, PgEmailChangeRepository, PgEmailVerificationRepository, PgEmojiRepository,
    PgFollowImportRepository, PgFollowRepository, PgHashtagRepository,
    PgInstanceDomainRepository, PgListRepository, PgMuteRepository, PgNotificationRepository,
    PgPasswordResetRepository, PgPinnedPostsRepository, PgPostRepository, PgReactionRepository,
    PgRelayRepository, PgRemoteEmojiRepository, PgRemoteInstanceMetaRepository, PgTotpRepository,
    PgUserRepository, PinnedPostsRepository, PostRepository, ReactionRepository, RelayRepository,
    RemoteEmojiRepository, RemoteInstanceMetaRepository, TotpRepository, UserRepository,
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
        if let Err(e) = sqlx::query(
            "UPDATE posts SET pending_bsky_media_file_id = $1 WHERE id = $2",
        )
        .bind(pending_media_file_id)
        .bind(post_id)
        .execute(&self.db)
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

    /// リモート Fedi アクターの followers/following 全件同期ジョブを積む（#68）。
    /// プロフィール表示時の短タイムアウト同期取得が失敗/タイムアウトした場合のフォールバック。
    ///
    /// #229: 同一 (actor_id, direction) を直近 [`REMOTE_FOLLOW_SYNC_COOLDOWN`] 以内に既に
    /// 積んでいれば再投入しない。フォロー数の多いアクターのプロフィールを何度もリロードする
    /// と、そのたびに最大5000件の`RemoteActorResolve`（優先度低）を積む重いジョブが重複投入
    /// され、同じ優先度を共有する他のジョブ（`AlsoKnownAsVerify`等）が飢餓状態になっていた。
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

    /// リモートフォロー一覧中の未知アクター（ローカルDB未登録）を解決するジョブを積む（#68）。
    pub async fn enqueue_remote_actor_resolve(&self, uri: String) {
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
    let actors: Arc<dyn ActorRepository> = Arc::new(PgActorRepository::new(pool.clone()));
    let users: Arc<dyn UserRepository> = Arc::new(PgUserRepository::new(pool.clone()));
    let posts: Arc<dyn PostRepository> = Arc::new(PgPostRepository::new(pool.clone()));
    let follows: Arc<dyn FollowRepository> = Arc::new(PgFollowRepository::new(pool.clone()));
    let follow_imports: Arc<dyn FollowImportRepository> =
        Arc::new(PgFollowImportRepository::new(pool.clone()));
    let blocks: Arc<dyn BlockRepository> = Arc::new(PgBlockRepository::new(pool.clone()));
    let mutes: Arc<dyn MuteRepository> = Arc::new(PgMuteRepository::new(pool.clone()));
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
        emojis,
        remote_emojis,
        relays,
        totp,
        webauthn,
    }
}

/// api ロールの axum ルーターを構築する（CORS 適用込み）。
pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // 管理系ルートはロールごとに専用ルータへ分割し、`route_layer`で認可を強制する（#221）。
    // ハンドラ側では認可チェックを一切行わない（呼び忘れによる無認可到達を構造的に防ぐ、
    // docs/code_audit_2026-08-05.md R-1）。
    let admin_router = Router::new()
        .route(
            "/api/admin/storage-providers",
            get(handlers::admin::storage::list_storage_providers)
                .post(handlers::admin::storage::create_storage_provider),
        )
        .route(
            "/api/admin/storage-providers/:id",
            patch(handlers::admin::storage::update_storage_provider)
                .delete(handlers::admin::storage::delete_storage_provider),
        )
        .route("/api/admin/users", get(handlers::admin::users::list_users))
        .route(
            "/api/admin/users/:id/suspend",
            post(handlers::admin::users::suspend_user),
        )
        .route(
            "/api/admin/users/:id/unsuspend",
            post(handlers::admin::users::unsuspend_user),
        )
        .route(
            "/api/admin/users/:id/role",
            post(handlers::admin::users::change_user_role),
        )
        .route(
            "/api/admin/users/:id/totp/disable",
            post(handlers::admin::users::disable_user_totp),
        )
        .route(
            "/api/admin/site-settings",
            get(handlers::admin::site_settings::get_site_settings)
                .patch(handlers::admin::site_settings::update_site_settings),
        )
        .route(
            "/api/admin/relays",
            get(handlers::admin::relays::list_relays).post(handlers::admin::relays::create_relay),
        )
        .route(
            "/api/admin/relays/:id",
            delete(handlers::admin::relays::delete_relay),
        )
        .route(
            "/api/admin/auth-ip-blocks",
            get(handlers::admin::auth_ip_blocks::list_ip_blocks),
        )
        .route(
            "/api/admin/auth-ip-blocks/:ip",
            delete(handlers::admin::auth_ip_blocks::unblock_ip),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::admin_only,
        ));

    let emoji_admin_router = Router::new()
        .route(
            "/api/admin/emojis",
            get(handlers::admin::emojis::list_emojis).post(handlers::admin::emojis::create_emoji),
        )
        .route(
            "/api/admin/emojis/:id",
            patch(handlers::admin::emojis::update_emoji)
                .delete(handlers::admin::emojis::delete_emoji),
        )
        // 絵文字インポート（#50）。多数のカスタム絵文字を含むZIPは数十〜数百MBになりうるため、
        // axum のデフォルトボディ上限（2MB）を明示的に引き上げる。
        .route(
            "/api/admin/emojis/import",
            post(handlers::admin::emoji_import::start_import)
                .layer(DefaultBodyLimit::max(200 * 1024 * 1024)),
        )
        .route(
            "/api/admin/emojis/import/:job_id",
            get(handlers::admin::emoji_import::get_import_status),
        )
        .route(
            "/api/admin/emojis/remote",
            get(handlers::admin::remote_emojis::list_remote_emojis),
        )
        .route(
            "/api/admin/emojis/remote/import",
            post(handlers::admin::remote_emojis::import_remote_emoji),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::emoji_admin_only,
        ));

    let report_moderator_router = Router::new()
        .route(
            "/api/admin/reports",
            get(handlers::admin::reports::list_reports),
        )
        .route(
            "/api/admin/reports/:id/close",
            post(handlers::admin::reports::close_report),
        )
        .route(
            "/api/admin/reports/:id/comments",
            get(handlers::admin::reports::list_comments)
                .post(handlers::admin::reports::add_comment),
        )
        .route(
            "/api/admin/reports/:id/delete-post",
            post(handlers::admin::reports::delete_subject_post),
        )
        .route(
            "/api/admin/reports/:id/suspend-user",
            post(handlers::admin::reports::suspend_subject),
        )
        .route(
            "/api/admin/reports/:id/forward",
            post(handlers::admin::reports::forward_report),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::report_moderator_only,
        ));

    Router::new()
        .merge(admin_router)
        .merge(emoji_admin_router)
        .merge(report_moderator_router)
        // ヘルスチェック（外形監視用、認証不要、#221監査R-9）
        .route("/health", get(handlers::health::health))
        // サイトアイコンを favicon として返す（#42）
        .route("/favicon.ico", get(handlers::favicon::favicon))
        .route(
            "/api/avatars/:actor_id",
            get(handlers::avatar::fallback_avatar),
        )
        // Misskey互換メディアプロキシ（リモート画像のCORS回避、SSRF防止付き）
        .route("/proxy", get(handlers::media_proxy::proxy))
        // セットアップ（初回管理者作成）
        .route("/api/setup/status", get(handlers::setup::setup_status))
        .route("/api/setup", post(handlers::setup::setup))
        // ユーザー通報
        .route("/api/reports", post(handlers::reports::create_report))
        // ドライブ（メディアアップロード）。動画・音声添付を考慮し 100MB まで許可
        // （axum のデフォルトボディ上限は小さいため明示的に上書きする）。
        .route(
            "/api/drive/files/create",
            post(handlers::drive::create_drive_file)
                .layer(DefaultBodyLimit::max(105 * 1024 * 1024)),
        )
        // 音声・動画の簡易視聴ページ（Bskyの外部リンクカードの参照先。直リンクだと
        // ダウンロードになってしまうため<audio>/<video>タグのみのHTMLを返す）
        .route(
            "/api/media/:media_file_id/watch",
            get(handlers::drive::watch_media),
        )
        // 認証
        .route(
            "/api/auth/verify-email",
            post(handlers::email_verify::request_email_verification),
        )
        .route(
            "/api/auth/verify-token",
            get(handlers::email_verify::verify_email_token),
        )
        .route("/api/auth/register", post(handlers::auth::register))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/me", get(handlers::auth::me))
        .route(
            "/api/auth/request-password-reset",
            post(handlers::auth::request_password_reset),
        )
        .route(
            "/api/auth/verify-reset-token",
            get(handlers::auth::verify_reset_token),
        )
        .route(
            "/api/auth/reset-password",
            post(handlers::auth::reset_password),
        )
        // TOTP（二段階認証、#65）: ログイン2段階目・認証アプリ紛失時のメール解除
        .route("/api/auth/totp/verify", post(handlers::totp::totp_verify))
        .route(
            "/api/auth/totp/request-disable-email",
            post(handlers::totp::totp_request_disable_email),
        )
        .route(
            "/api/auth/totp/confirm-disable",
            post(handlers::totp::totp_confirm_disable),
        )
        .route(
            "/api/auth/passkeys/start",
            post(handlers::passkeys::authentication_start),
        )
        .route(
            "/api/auth/passkeys/finish",
            post(handlers::passkeys::authentication_finish),
        )
        // アカウント管理（退会等）
        .route("/api/account/withdraw", post(handlers::account::withdraw))
        .route(
            "/api/account/change-password",
            post(handlers::account::change_password),
        )
        .route(
            "/api/account/revoke-all-sessions",
            post(handlers::account::revoke_all_sessions),
        )
        .route(
            "/api/account/language",
            post(handlers::account::update_language),
        )
        .route(
            "/api/account/content-visibility",
            get(handlers::account::get_content_visibility)
                .post(handlers::account::update_content_visibility),
        )
        .route(
            "/api/account/email/request-change",
            post(handlers::account::request_email_change),
        )
        .route(
            "/api/account/email/confirm-change",
            post(handlers::account::confirm_email_change),
        )
        .route(
            "/api/account/app-tokens",
            get(handlers::account::list_app_tokens).post(handlers::account::create_app_token),
        )
        .route(
            "/api/account/app-tokens/:id",
            delete(handlers::account::revoke_app_token),
        )
        // TOTP（二段階認証、#65）: 設定画面での有効化・無効化
        .route("/api/account/totp/status", get(handlers::totp::totp_status))
        .route("/api/account/totp/setup", post(handlers::totp::totp_setup))
        .route(
            "/api/account/totp/enable",
            post(handlers::totp::totp_enable),
        )
        .route(
            "/api/account/totp/disable",
            post(handlers::totp::totp_disable),
        )
        .route("/api/account/passkeys", get(handlers::passkeys::list))
        .route(
            "/api/account/passkeys/registration/start",
            post(handlers::passkeys::registration_start),
        )
        .route(
            "/api/account/passkeys/registration/finish",
            post(handlers::passkeys::registration_finish),
        )
        .route(
            "/api/account/passkeys/:id",
            delete(handlers::passkeys::delete),
        )
        // 投稿
        .route("/api/notes/create", post(handlers::notes::create_note))
        .route(
            "/api/notes/local-timeline",
            get(handlers::notes::local_timeline),
        )
        .route(
            "/api/notes/home-timeline",
            get(handlers::notes::home_timeline),
        )
        .route(
            "/api/notes/social-timeline",
            get(handlers::notes::social_timeline),
        )
        // Misskeyクライアント（Aria等）は`/api/notes/global-timeline`をPOSTで叩く（#78）。GET/POST共存。
        .route(
            "/api/notes/global-timeline",
            get(handlers::notes::global_timeline)
                .post(handlers::misskey::endpoints::notes_global_timeline),
        )
        // Misskey 互換エイリアス
        .route("/api/notes/timeline", get(handlers::notes::home_timeline))
        .route(
            "/api/notes/search",
            get(handlers::search::search_notes).post(handlers::misskey::endpoints::notes_search),
        )
        .route(
            "/api/notes/search-by-tag",
            post(handlers::misskey::endpoints::notes_search_by_tag),
        )
        .route("/api/open", post(handlers::open_target::open_target))
        // ダイレクトメッセージ（DM本体の送受信は既存の /api/notes/create を再利用する）
        .route("/api/dm/sessions", get(handlers::dm::sessions))
        .route(
            "/api/dm/sessions/:thread_root_id/messages",
            get(handlers::dm::thread_messages),
        )
        .route(
            "/api/dm/sessions/:thread_root_id/read",
            post(handlers::dm::mark_read),
        )
        .route("/api/dm/unread-count", get(handlers::dm::unread_count))
        .route("/api/streaming", get(handlers::streaming::streaming))
        .route(
            "/api/notes/:id",
            get(handlers::notes::get_note).delete(handlers::notes::delete_note),
        )
        .route(
            "/api/notes/:id/repost",
            delete(handlers::notes::delete_repost),
        )
        .route(
            "/api/reactions/frequent",
            get(handlers::notes::frequent_reactions),
        )
        .route(
            "/api/notes/:id/reactions",
            post(handlers::notes::create_reaction),
        )
        .route("/api/notes/:id/poll-vote", post(handlers::notes::vote_poll))
        .route(
            "/api/notes/:id/reactions/:content",
            delete(handlers::notes::delete_reaction),
        )
        .route(
            "/api/notes/:id/reactions/:content/actors",
            get(handlers::notes::reaction_actors),
        )
        .route("/api/notes/:id/pin", post(handlers::notes::pin_note))
        .route("/api/notes/:id/pin", delete(handlers::notes::unpin_note))
        .route("/api/notes/:id/context", get(handlers::notes::note_context))
        .route("/api/notes/:id/replies", get(handlers::notes::note_replies))
        .route("/api/notes/:id/reposts", get(handlers::notes::note_reposts))
        // ActivityPub Note / OGP注入済みSPA（Accept ヘッダーで振り分け、`handlers::ogp`）
        .route("/notes/:id", get(handlers::notes::get_note_ap))
        // Announce（リポストラッパー）canonical URL。ブラウザは /notes/:id へリダイレクト
        .route(
            "/announces/:id",
            get(handlers::notes::get_announce_redirect),
        )
        // プロフィールページ（OGP注入済みSPA HTMLを返す、`handlers::ogp`）
        .route("/@:handle", get(handlers::ogp::profile_ogp))
        // フォロー
        .route(
            "/api/follows/create",
            post(handlers::follows::create_follow),
        )
        .route(
            "/api/follows/delete",
            post(handlers::follows::delete_follow),
        )
        // フォローインポート（設定画面から改行区切りのID一覧を貼り付けて一括フォロー）
        .route(
            "/api/account/follow-import",
            post(handlers::follow_import::start_import).get(handlers::follow_import::get_status),
        )
        .route(
            "/api/account/follow-import/cancel",
            post(handlers::follow_import::cancel_import),
        )
        // ブロック（Bsky準拠：フォロー強制解除＋相互完全非表示。Fediへは Block 配送、Bskyへは app.bsky.graph.block をコミット）
        .route("/api/blocks/create", post(handlers::blocks::create_block))
        .route("/api/blocks/delete", post(handlers::blocks::delete_block))
        .route("/api/blocks", get(handlers::blocks::list_blocks))
        // ミュート（ローカル効果のみ、AP/ATP配送なし）
        .route("/api/mutes/create", post(handlers::mutes::create_mute))
        .route("/api/mutes/delete", post(handlers::mutes::delete_mute))
        .route("/api/mutes", get(handlers::mutes::list_mutes))
        // リスト（#63）
        .route(
            "/api/lists",
            get(handlers::lists::my_lists).post(handlers::lists::create_list),
        )
        .route(
            "/api/lists/:id",
            get(handlers::lists::get_list)
                .patch(handlers::lists::update_list)
                .delete(handlers::lists::delete_list),
        )
        .route("/api/lists/:id/members", post(handlers::lists::add_member))
        .route(
            "/api/lists/:id/members/:actor_id",
            delete(handlers::lists::remove_member),
        )
        .route(
            "/api/lists/:id/timeline",
            get(handlers::lists::list_timeline),
        )
        // ハッシュタグ
        .route(
            "/api/hashtags/pinned",
            get(handlers::hashtags::pinned_hashtags),
        )
        .route(
            "/api/hashtags/:name/timeline",
            get(handlers::hashtags::hashtag_timeline),
        )
        .route(
            "/api/hashtags/:name/pin",
            post(handlers::hashtags::pin_hashtag).delete(handlers::hashtags::unpin_hashtag),
        )
        .route(
            "/api/actors/search",
            get(handlers::actor_search::search_actors),
        )
        .route(
            "/api/actors/suggest",
            get(handlers::actor_search::suggest_actors),
        )
        // ユーザープロフィール
        .route(
            "/api/users/profile",
            get(handlers::users::user_profile).patch(handlers::users::update_profile),
        )
        .route("/api/users/posts", get(handlers::users::user_posts))
        // Misskey クライアント（Aria等）は同パスをPOSTで叩く（#81）。GET/POST共存。
        .route(
            "/api/users/following",
            get(handlers::users::user_following)
                .post(handlers::misskey::endpoints::users_following),
        )
        .route(
            "/api/users/followers",
            get(handlers::users::user_followers)
                .post(handlers::misskey::endpoints::users_followers),
        )
        .route(
            "/api/users/remote-follow-summary",
            get(handlers::users::user_remote_follow_summary),
        )
        // プロフィールの「別のアカウント」（alsoKnownAs、seiran独自拡張）
        .route(
            "/api/users/also-known-as",
            post(handlers::also_known_as::add),
        )
        .route(
            "/api/users/also-known-as/:actor_id",
            delete(handlers::also_known_as::remove),
        )
        // Misskey 互換レイヤー
        .route("/api/meta", post(handlers::meta::api_meta))
        .route(
            "/api/endpoints",
            post(handlers::misskey::endpoints::endpoints),
        )
        // カスタム絵文字一覧（未認証・Misskey クライアントのリアクションピッカー用）
        // Misskey 本家は `allowGet: true` でGET/POST両対応。Aria 等のクライアントは
        // POST で呼ぶため、GET のみだと 405 Method Not Allowed になり絵文字が出ない。
        .route(
            "/api/emojis",
            get(handlers::emojis::list_emojis).post(handlers::emojis::list_emojis),
        )
        // Misskey 準拠の追加エンドポイント（Phase 2）。既存のカスタムAPIと並存する。
        .route("/api/i", post(handlers::misskey::endpoints::api_i))
        .route(
            "/api/users/show",
            post(handlers::misskey::endpoints::users_show),
        )
        .route(
            "/api/users/notes",
            post(handlers::misskey::endpoints::users_notes),
        )
        .route(
            "/api/notes/show",
            post(handlers::misskey::endpoints::notes_show),
        )
        .route(
            "/api/notes/local-timeline",
            post(handlers::misskey::endpoints::notes_local_timeline),
        )
        .route(
            "/api/notes/timeline",
            post(handlers::misskey::endpoints::notes_home_timeline),
        )
        .route(
            "/api/notes/reactions",
            post(handlers::misskey::endpoints::notes_reactions),
        )
        .route(
            "/api/notes/hybrid-timeline",
            post(handlers::misskey::endpoints::notes_hybrid_timeline),
        )
        .route(
            "/api/notes/reactions/create",
            post(handlers::misskey::endpoints::reactions_create),
        )
        .route(
            "/api/notes/reactions/delete",
            post(handlers::misskey::endpoints::reactions_delete),
        )
        .route(
            "/api/notes/unrenote",
            post(handlers::misskey::endpoints::notes_unrenote),
        )
        .route(
            "/api/following/create",
            post(handlers::misskey::endpoints::following_create),
        )
        .route(
            "/api/following/delete",
            post(handlers::misskey::endpoints::following_delete),
        )
        .route(
            "/api/i/notifications",
            post(handlers::misskey::endpoints::i_notifications),
        )
        // MiAuth（Misskey 互換クライアント用）
        .route("/miauth/:session_id", get(handlers::miauth::miauth_page))
        .route(
            "/api/miauth/:session_id/authorize",
            post(handlers::miauth::miauth_authorize),
        )
        .route(
            "/api/miauth/:session_id/check",
            post(handlers::miauth::miauth_check_by_path),
        )
        .route("/api/miauth/check", post(handlers::miauth::miauth_check))
        // AT Protocol XRPC エンドポイント
        .route(
            "/xrpc/com.atproto.server.describeServer",
            get(handlers::xrpc::server::xrpc_describe_server),
        )
        .route(
            "/xrpc/com.atproto.identity.resolveHandle",
            get(handlers::xrpc::server::xrpc_resolve_handle),
        )
        .route(
            "/xrpc/com.atproto.sync.getRepo",
            get(handlers::xrpc::sync::xrpc_get_repo),
        )
        .route(
            "/xrpc/com.atproto.sync.getBlob",
            get(handlers::xrpc::sync::xrpc_get_blob),
        )
        .route(
            "/xrpc/com.atproto.sync.subscribeRepos",
            get(handlers::xrpc::sync::xrpc_subscribe_repos),
        )
        .route(
            "/xrpc/com.atproto.repo.getRecord",
            get(handlers::xrpc::repo::xrpc_get_record),
        )
        .route(
            "/xrpc/com.atproto.repo.listRecords",
            get(handlers::xrpc::repo::xrpc_list_records),
        )
        .route(
            "/xrpc/com.atproto.repo.describeRepo",
            get(handlers::xrpc::repo::xrpc_describe_repo),
        )
        .route(
            "/xrpc/com.atproto.repo.createRecord",
            post(handlers::xrpc::repo::xrpc_create_record),
        )
        .route(
            "/xrpc/com.atproto.repo.putRecord",
            post(handlers::xrpc::repo::xrpc_put_record),
        )
        .route(
            "/xrpc/com.atproto.repo.deleteRecord",
            post(handlers::xrpc::repo::xrpc_delete_record),
        )
        .route(
            "/xrpc/com.atproto.repo.applyWrites",
            post(handlers::xrpc::repo::xrpc_apply_writes),
        )
        .route(
            "/xrpc/com.atproto.sync.listRepos",
            get(handlers::xrpc::sync::xrpc_list_repos),
        )
        .route(
            "/xrpc/com.atproto.sync.getLatestCommit",
            get(handlers::xrpc::sync::xrpc_get_latest_commit),
        )
        .route(
            "/xrpc/com.atproto.sync.listBlobs",
            get(handlers::xrpc::sync::xrpc_list_blobs),
        )
        .route(
            "/xrpc/com.atproto.server.createSession",
            post(handlers::xrpc::server::xrpc_create_session),
        )
        .route(
            "/xrpc/com.atproto.server.refreshSession",
            post(handlers::xrpc::server::xrpc_refresh_session),
        )
        .route(
            "/xrpc/com.atproto.server.deleteSession",
            post(handlers::xrpc::server::xrpc_delete_session),
        )
        .route(
            "/xrpc/com.atproto.server.getSession",
            get(handlers::xrpc::server::xrpc_get_session),
        )
        .route(
            "/xrpc/app.bsky.actor.getPreferences",
            get(handlers::xrpc::actor::xrpc_get_preferences),
        )
        .route(
            "/xrpc/app.bsky.actor.putPreferences",
            post(handlers::xrpc::actor::xrpc_put_preferences),
        )
        .route(
            "/xrpc/com.atproto.server.createAppPassword",
            post(handlers::xrpc::server::xrpc_create_app_password),
        )
        .route(
            "/xrpc/com.atproto.server.listAppPasswords",
            get(handlers::xrpc::server::xrpc_list_app_passwords),
        )
        .route(
            "/xrpc/com.atproto.server.revokeAppPassword",
            post(handlers::xrpc::server::xrpc_revoke_app_password),
        )
        // Bsky公式動画パイプライン（uploadVideo）が完了後に呼び戻してくるコールバック
        .route(
            "/xrpc/com.atproto.repo.uploadBlob",
            post(handlers::xrpc::repo::xrpc_upload_blob),
        )
        // AT Protocol DID 解決
        .route(
            "/.well-known/did.json",
            get(handlers::xrpc::server::well_known_did),
        )
        .route(
            "/.well-known/atproto-did",
            get(handlers::xrpc::server::well_known_atproto_did),
        )
        // 未実装のXRPCメソッドへの `atproto-proxy` ヘッダー付きリクエストをAppView等へ
        // 透過転送する（明示的な `.route()` の方が優先されるため、ここに置いても既存の
        // XRPCハンドラを妨げない）。
        .fallback(handlers::xrpc::proxy::xrpc_proxy_fallback)
        .with_state(state)
        // Misskey クライアントの `i`（ボディ/クエリ）トークンを Authorization ヘッダーへ
        // 合成するブリッジ。既存ハンドラの extract_auth 呼び出しは無改修のまま両対応になる。
        .layer(axum::middleware::from_fn(
            middleware::misskey_auth_bridge::bridge,
        ))
        .layer(cors)
}

/// 起動時タスク: 全ローカルユーザーの Cloudflare TXT 再登録 → Relay requestCrawl →
/// #identity イベントのバックフィル、をこの順でバックグラウンド実行する。
pub fn spawn_startup_tasks(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        resume_running_follow_imports(&state).await;
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
    let request_ids: Vec<i64> = match sqlx::query_scalar(
        "SELECT id FROM follow_import_requests WHERE status = 'running'",
    )
    .fetch_all(&state.db)
    .await
    {
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

/// 起動時リカバリ: プロセス再起動で停止した退会時一括アンフォロー（`Job::AccountWithdrawUnfollowAll`）
/// を再開する。`actors.withdrawn_at` が設定済み（退会済み）なのに `follows` にまだ
/// フォロー先が残っているアクターを検出し、無条件で全件再enqueueする（`resume_running_follow_imports`
/// と同じ理由で絞り込まない）。重複投入は`jobs::account_withdraw_unfollow_all`の
/// `actor_id` 単位 advisory lock が解消する。
async fn resume_account_withdraw_unfollow_all(state: &AppState) {
    let rows: Vec<(i64, String)> = match sqlx::query_as(
        "SELECT a.id, a.username FROM actors a
         WHERE a.withdrawn_at IS NOT NULL
           AND EXISTS (SELECT 1 FROM follows f WHERE f.follower_actor_id = a.id)",
    )
    .fetch_all(&state.db)
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
    let media_file_ids: Vec<i64> = match sqlx::query_scalar(
        "SELECT id FROM media_files WHERE bsky_video_status = 'pending'",
    )
    .fetch_all(&state.db)
    .await
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
    let rows: Vec<(i64, i64, i64)> = match sqlx::query_as(
        "SELECT id, actor_id, pending_bsky_media_file_id FROM posts
         WHERE pending_bsky_media_file_id IS NOT NULL AND at_uri IS NULL",
    )
    .fetch_all(&state.db)
    .await
    {
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
/// `icon_url`/`node_name`がNULLの行も対象に含める: サーバーアイコン取得・
/// `<title>`タグフォールバック機能をそれぞれ後から追加した際、それ以前に解決済み
/// だった行（列自体は追加されているが値は未取得）が`NOT EXISTS`だけの判定だと
/// 永久に再取得されず放置される事故があったため（2026-08-19実機確認、misskey.dev等の
/// 主要インスタンスがこれで固定的に🌐表示・ドメイン名表示のままになった）。
/// 非対応サーバーは毎回再チャレンジすることになるが、起動時のみの発生でありコストは小さい。
async fn backfill_remote_instance_meta(state: &AppState) {
    let domains = match sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT a.domain FROM actors a
         WHERE a.actor_type IN ('fedi', 'remote_seiran') AND a.domain != ''
           AND NOT EXISTS (
               SELECT 1 FROM remote_instance_meta rim
               WHERE rim.domain = a.domain
                 AND rim.icon_url IS NOT NULL
                 AND rim.node_name IS NOT NULL
           )",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[startup] remote_instance_meta backfill対象取得失敗: {}", e);
            return;
        }
    };

    let total = domains.len();
    for domain in domains {
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

    let actor_ids = match sqlx::query_scalar::<_, i64>(
        "SELECT id FROM actors
         WHERE actor_type = 'local' AND avatar_media_id IS NULL AND at_did IS NOT NULL
         ORDER BY id",
    )
    .fetch_all(&state.db)
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
            Ok((display_name, description, _)) => match state
                .atp_service
                .commit_profile(
                    actor_id,
                    &display_name,
                    description.as_deref(),
                    None,
                    pinned_post,
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
    let missing: Vec<(i64, String, String)> = match sqlx::query_as::<_, (i64, String, String)>(
        "SELECT a.id, a.username, a.at_did
         FROM actors a
         WHERE a.actor_type = 'local' AND a.at_did IS NOT NULL
           AND NOT EXISTS (
             SELECT 1 FROM atp_repo_events e
             WHERE e.actor_id = a.id AND e.event_type = 'identity'
           )",
    )
    .fetch_all(&state.db)
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
    let missing: Vec<i64> = match sqlx::query_scalar::<_, i64>(
        "SELECT a.id
         FROM actors a
         WHERE a.actor_type = 'local' AND a.at_did IS NOT NULL
           AND NOT EXISTS (
             SELECT 1 FROM atp_records r
             WHERE r.actor_id = a.id AND r.collection = 'chat.bsky.actor.declaration' AND r.rkey = 'self'
           )",
    )
    .fetch_all(&state.db)
    .await
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

// =====================================================================
// メディア GC タスク
// =====================================================================

/// アップロードされたが参照されていない media_files を定期的に削除するタスク。
///
/// 1時間ごとに孤立ファイル（7日以上経過かつどのテーブルからも参照なし）を
/// S3 → DB の順でベストエフォートで削除する。
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
    let media_files = Arc::clone(&state.media_files);
    let storage_providers = Arc::clone(&state.storage_providers);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            run_media_gc(&db, media_files.as_ref(), storage_providers.as_ref()).await;
        }
    });

    // atp_blobs（uploadBlob 受信バイト列。Bsky動画パイプラインの代理POST等）のGC。
    // media_files と同じ7日ルールで、どの media_files.bsky_video_cid からも
    // 参照されなくなったものを削除する（2026-07-17 マイケル指摘: 無制限保存の防止）。
    let db2 = state.db.clone();
    let storage_providers2 = Arc::clone(&state.storage_providers);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600));
        loop {
            interval.tick().await;
            run_atp_blobs_gc(&db2, storage_providers2.as_ref()).await;
        }
    });
}

/// 孤立メディアファイルを保持する中間構造体。
#[derive(sqlx::FromRow)]
struct OrphanedMediaFile {
    id: i64,
    storage_provider_id: i64,
    storage_key: String,
}

/// 孤立ファイルを最大 100 件取得し、S3 → DB の順で削除する（ベストエフォート）。
async fn run_media_gc(
    pool: &sqlx::PgPool,
    media_files: &dyn MediaFileRepository,
    storage_providers: &dyn StorageProviderRepository,
) {
    let rows: Vec<OrphanedMediaFile> = match sqlx::query_as::<_, OrphanedMediaFile>(
        "SELECT id, storage_provider_id, storage_key
         FROM media_files
         WHERE created_at < NOW() - INTERVAL '7 days'
           AND id NOT IN (SELECT media_file_id FROM post_attachments)
           AND id NOT IN (SELECT avatar_media_id FROM actors WHERE avatar_media_id IS NOT NULL)
           AND id NOT IN (SELECT banner_media_id FROM actors WHERE banner_media_id IS NOT NULL)
           AND id NOT IN (SELECT media_file_id FROM custom_emojis)
         LIMIT 100",
    )
    .fetch_all(pool)
    .await
    {
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
        match storage_providers.find_by_id(row.storage_provider_id).await {
            Ok(Some(provider)) => {
                let s3 = S3StorageClient::new(&provider);
                if let Err(e) = s3.delete(&row.storage_key).await {
                    tracing::error!("[media-gc] S3 削除失敗 id={}: {}", row.id, e);
                    continue; // S3 失敗時は DB も削除しない
                }
                if let Err(e) = media_files.delete_by_id(row.id).await {
                    tracing::error!("[media-gc] DB 削除失敗 id={}: {}", row.id, e);
                } else {
                    tracing::info!("[media-gc] 削除完了 id={}", row.id);
                }
            }
            Ok(None) => {
                tracing::warn!(
                    "[media-gc] プロバイダー不明 id={}, provider_id={}",
                    row.id,
                    row.storage_provider_id
                );
            }
            Err(e) => {
                tracing::error!("[media-gc] プロバイダー取得失敗: {}", e);
            }
        }
    }
}

/// 孤立 atp_blobs（7日以上経過し、どの `media_files.bsky_video_cid` からも
/// 参照されていない）を最大100件取得し、S3 → DB の順で削除する（ベストエフォート）。
async fn run_atp_blobs_gc(pool: &sqlx::PgPool, storage_providers: &dyn StorageProviderRepository) {
    let rows: Vec<OrphanedMediaFile> = match sqlx::query_as::<_, OrphanedMediaFile>(
        "SELECT id, storage_provider_id, storage_key
         FROM atp_blobs
         WHERE created_at < NOW() - INTERVAL '7 days'
           AND cid NOT IN (SELECT bsky_video_cid FROM media_files WHERE bsky_video_cid IS NOT NULL)
         LIMIT 100",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[atp-blobs-gc] 孤立ブロブ取得失敗: {}", e);
            return;
        }
    };

    if rows.is_empty() {
        return;
    }
    tracing::info!("[atp-blobs-gc] 孤立ブロブ {} 件を処理します", rows.len());

    for row in rows {
        match storage_providers.find_by_id(row.storage_provider_id).await {
            Ok(Some(provider)) => {
                let s3 = S3StorageClient::new(&provider);
                if let Err(e) = s3.delete(&row.storage_key).await {
                    tracing::error!("[atp-blobs-gc] S3 削除失敗 id={}: {}", row.id, e);
                    continue;
                }
                if let Err(e) = sqlx::query("DELETE FROM atp_blobs WHERE id = $1")
                    .bind(row.id)
                    .execute(pool)
                    .await
                {
                    tracing::error!("[atp-blobs-gc] DB 削除失敗 id={}: {}", row.id, e);
                } else {
                    tracing::info!("[atp-blobs-gc] 削除完了 id={}", row.id);
                }
            }
            Ok(None) => {
                tracing::warn!(
                    "[atp-blobs-gc] プロバイダー不明 id={}, provider_id={}",
                    row.id,
                    row.storage_provider_id
                );
            }
            Err(e) => {
                tracing::error!("[atp-blobs-gc] プロバイダー取得失敗: {}", e);
            }
        }
    }
}
