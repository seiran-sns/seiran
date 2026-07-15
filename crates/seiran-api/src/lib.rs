//! seiran-api — REST API / 認証 / タイムライン / XRPC を提供するライブラリ。
//!
//! バイナリは `seiran-server` が `--role api`（または `all`）で起動する。
//! ここでは AppState 構築（[`init_state`]）・ルーター構築（[`router`]）・
//! 起動時タスク（[`spawn_startup_tasks`]）を公開し、実際の serve は呼び出し側が行う。

pub mod cloudflare;
pub mod error;
pub mod mailer;
pub mod middleware;
pub mod handlers;
pub mod search;
pub mod streaming;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use dashmap::DashMap;
use tower_http::cors::{Any, CorsLayer};
use axum::{extract::DefaultBodyLimit, routing::{delete, get, patch, post}, Router};
use sqlx::PgPool;

use seiran_common::{
    LocalAuthProvider, Secrets, AtpCommitService, AtpCommitEvent, ApClient,
    StorageProviderRepository, PgStorageProviderRepository,
    MediaFileRepository, PgMediaFileRepository,
    SiteSettingsRepository, PgSiteSettingsRepository,
    S3StorageClient, ApDeliveryKind, Job, JobQueue, job_priority,
};
use seiran_common::repository::{
    ActorRepository, AtpReadRepository, FollowRepository, NotificationRepository, PinnedPostsRepository, PostRepository, ReactionRepository, UserRepository,
    PgActorRepository, PgAtpReadRepository, PgFollowRepository, PgNotificationRepository, PgPinnedPostsRepository, PgPostRepository, PgReactionRepository, PgUserRepository,
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
    pub users: Arc<dyn UserRepository>,
    pub posts: Arc<dyn PostRepository>,
    pub follows: Arc<dyn FollowRepository>,
    pub atp_repo: Arc<dyn AtpReadRepository>,
    /// リアクション（絵文字リアクション・いいね）リポジトリ。
    pub reactions: Arc<dyn ReactionRepository>,
    /// ピン留めポスト（ローカルユーザーの pin/unpin 操作結果 + リモートアクターの同期結果の共通ストア）。
    pub pinned_posts: Arc<dyn PinnedPostsRepository>,
    /// 通知（フォロー・リアクション等）の永続化リポジトリ。
    pub notifications: Arc<dyn NotificationRepository>,
    /// deliver_post_to_ap_followers（seiran-common）が &PgPool を要求するため保持。
    /// 将来 FollowerRepository へ移行したら削除する。
    pub db: PgPool,
    pub local_auth: Arc<LocalAuthProvider>,
    pub miauth_sessions: Arc<RwLock<HashMap<String, MiAuthSession>>>,
    pub local_domain: String,
    pub secrets: Arc<Secrets>,
    pub atp_service: Arc<AtpCommitService>,
    pub http_client: Arc<reqwest::Client>,
    pub ap_client: Arc<ApClient>,
    pub cloudflare: Option<Arc<cloudflare::CloudflareClient>>,
    pub storage_providers: Arc<dyn StorageProviderRepository>,
    pub media_files: Arc<dyn MediaFileRepository>,
    pub site_settings: Arc<dyn SiteSettingsRepository>,
    pub search_store: Arc<InMemorySearchStore>,
    /// リアルタイム更新（#37）のストリーミングハブ。
    pub stream_hub: Arc<StreamHub>,
    /// 絵文字インポートジョブの進捗状態（#50）。job_id → ImportJobStatus。
    pub emoji_import_jobs: Arc<DashMap<String, handlers::admin::emoji_import::ImportJobStatus>>,
    /// 非同期ジョブキュー（AP配送・Bsky動画パイプライン結合等）。`all` ロールでは
    /// `seiran-federation-worker`のWorkerEngineと同一インスタンスを共有する。
    pub job_queue: Arc<dyn JobQueue>,
}

impl AppState {
    /// AP 配送ジョブを積む。配送の実行・リトライは Worker（`jobs::ap_delivery`）が担う。
    /// enqueue 失敗はログのみ（投稿等の主処理は成功済みのため呼び出し元へは伝播しない）。
    pub async fn enqueue_ap_delivery(&self, actor_id: i64, kind: ApDeliveryKind) {
        if let Err(e) = self
            .job_queue
            .enqueue(Job::ApDelivery { actor_id, kind }, job_priority::HIGH)
            .await
        {
            tracing::error!("[job] ApDelivery enqueue 失敗 (actor_id={}): {}", actor_id, e);
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
    local_domain: String,
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

    let mut atp_service =
        AtpCommitService::new(pool.clone(), Arc::clone(&atp_event_tx), Arc::clone(&http_client));
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
    let actors: Arc<dyn ActorRepository> = Arc::new(PgActorRepository::new(pool.clone()));
    let users: Arc<dyn UserRepository> = Arc::new(PgUserRepository::new(pool.clone()));
    let posts: Arc<dyn PostRepository> = Arc::new(PgPostRepository::new(pool.clone()));
    let follows: Arc<dyn FollowRepository> = Arc::new(PgFollowRepository::new(pool.clone()));
    let atp_repo: Arc<dyn AtpReadRepository> = Arc::new(PgAtpReadRepository::new(pool.clone()));
    let reactions: Arc<dyn ReactionRepository> = Arc::new(PgReactionRepository::new(pool.clone()));
    let pinned_posts: Arc<dyn PinnedPostsRepository> = Arc::new(PgPinnedPostsRepository::new(pool.clone()));
    let notifications: Arc<dyn NotificationRepository> = Arc::new(PgNotificationRepository::new(pool.clone()));

    AppState {
        actors,
        users,
        posts,
        follows,
        atp_repo,
        reactions,
        pinned_posts,
        notifications,
        db: pool,
        local_auth,
        miauth_sessions: Arc::new(RwLock::new(HashMap::new())),
        local_domain,
        secrets,
        atp_service,
        http_client,
        ap_client,
        cloudflare,
        storage_providers,
        media_files,
        site_settings,
        search_store: Arc::new(InMemorySearchStore::new()),
        stream_hub: Arc::new(StreamHub::new()),
        emoji_import_jobs: Arc::new(DashMap::new()),
        job_queue,
    }
}

/// api ロールの axum ルーターを構築する（CORS 適用込み）。
pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // サイトアイコンを favicon として返す（#42）
        .route("/favicon.ico", get(handlers::favicon::favicon))
        // セットアップ（初回管理者作成）
        .route("/api/setup/status", get(handlers::setup::setup_status))
        .route("/api/setup", post(handlers::setup::setup))
        // 管理者 API
        .route("/api/admin/storage-providers",
            get(handlers::admin::storage::list_storage_providers)
            .post(handlers::admin::storage::create_storage_provider))
        .route("/api/admin/storage-providers/:id",
            patch(handlers::admin::storage::update_storage_provider)
            .delete(handlers::admin::storage::delete_storage_provider))
        // 管理者ユーザー管理
        .route("/api/admin/users", get(handlers::admin::users::list_users))
        .route("/api/admin/users/:id/suspend", post(handlers::admin::users::suspend_user))
        .route("/api/admin/users/:id/unsuspend", post(handlers::admin::users::unsuspend_user))
        .route("/api/admin/users/:id/role", post(handlers::admin::users::change_user_role))
        // サイト設定
        .route("/api/admin/site-settings",
            get(handlers::admin::site_settings::get_site_settings)
            .patch(handlers::admin::site_settings::update_site_settings))
        // カスタム絵文字
        .route("/api/admin/emojis",
            get(handlers::admin::emojis::list_emojis)
            .post(handlers::admin::emojis::create_emoji))
        .route("/api/admin/emojis/:id",
            patch(handlers::admin::emojis::update_emoji)
            .delete(handlers::admin::emojis::delete_emoji))
        // 絵文字インポート（#50）
        .route("/api/admin/emojis/import",
            post(handlers::admin::emoji_import::start_import))
        .route("/api/admin/emojis/import/:job_id",
            get(handlers::admin::emoji_import::get_import_status))
        // ドライブ（メディアアップロード）。動画・音声添付を考慮し 100MB まで許可
        // （axum のデフォルトボディ上限は小さいため明示的に上書きする）。
        .route(
            "/api/drive/files/create",
            post(handlers::drive::create_drive_file)
                .layer(DefaultBodyLimit::max(105 * 1024 * 1024)),
        )
        // 認証
        .route("/api/auth/verify-email", post(handlers::email_verify::request_email_verification))
        .route("/api/auth/verify-token", get(handlers::email_verify::verify_email_token))
        .route("/api/auth/register", post(handlers::auth::register))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/me", get(handlers::auth::me))
        .route("/api/auth/request-password-reset", post(handlers::auth::request_password_reset))
        .route("/api/auth/verify-reset-token", get(handlers::auth::verify_reset_token))
        .route("/api/auth/reset-password", post(handlers::auth::reset_password))
        // アカウント管理（退会等）
        .route("/api/account/withdraw", post(handlers::account::withdraw))
        // 投稿
        .route("/api/notes/create", post(handlers::notes::create_note))
        .route("/api/notes/local-timeline", get(handlers::notes::local_timeline))
        .route("/api/notes/home-timeline", get(handlers::notes::home_timeline))
        // Misskey 互換エイリアス
        .route("/api/notes/timeline", get(handlers::notes::home_timeline))
        .route("/api/notes/search", get(handlers::search::search_notes))
        .route("/api/streaming", get(handlers::streaming::streaming))
        .route("/api/notes/:id", get(handlers::notes::get_note))
        .route("/api/notes/:id/repost", delete(handlers::notes::delete_repost))
        .route("/api/notes/:id/reactions", post(handlers::notes::create_reaction))
        .route("/api/notes/:id/reactions/:content", delete(handlers::notes::delete_reaction))
        .route("/api/notes/:id/pin", post(handlers::notes::pin_note))
        .route("/api/notes/:id/pin", delete(handlers::notes::unpin_note))
        .route("/api/notes/:id/context", get(handlers::notes::note_context))
        // ActivityPub Note エンドポイント（nginx が AP Accept ヘッダーのみをここへ転送）
        .route("/notes/:id", get(handlers::notes::get_note_ap))
        // フォロー
        .route("/api/follows/create", post(handlers::follows::create_follow))
        .route("/api/follows/delete", post(handlers::follows::delete_follow))
        // ユーザープロフィール
        .route("/api/users/profile",
            get(handlers::users::user_profile)
            .patch(handlers::users::update_profile))
        // Misskey 互換レイヤー
        .route("/api/meta", post(handlers::meta::api_meta))
        // カスタム絵文字一覧（未認証・Misskey クライアントのリアクションピッカー用）
        .route("/api/emojis", get(handlers::emojis::list_emojis))
        // Misskey 準拠の追加エンドポイント（Phase 2）。既存のカスタムAPIと並存する。
        .route("/api/i", post(handlers::misskey::endpoints::api_i))
        .route("/api/users/show", post(handlers::misskey::endpoints::users_show))
        .route("/api/notes/show", post(handlers::misskey::endpoints::notes_show))
        .route("/api/notes/local-timeline", post(handlers::misskey::endpoints::notes_local_timeline))
        .route("/api/notes/timeline", post(handlers::misskey::endpoints::notes_home_timeline))
        .route("/api/notes/reactions/create", post(handlers::misskey::endpoints::reactions_create))
        .route("/api/notes/reactions/delete", post(handlers::misskey::endpoints::reactions_delete))
        .route("/api/notes/unrenote", post(handlers::misskey::endpoints::notes_unrenote))
        .route("/api/following/create", post(handlers::misskey::endpoints::following_create))
        .route("/api/following/delete", post(handlers::misskey::endpoints::following_delete))
        .route("/api/i/notifications", post(handlers::misskey::endpoints::i_notifications))
        // MiAuth（Misskey 互換クライアント用）
        .route("/miauth/:session_id", get(handlers::miauth::miauth_page))
        .route("/api/miauth/:session_id/authorize", post(handlers::miauth::miauth_authorize))
        .route("/api/miauth/:session_id/check", post(handlers::miauth::miauth_check_by_path))
        .route("/api/miauth/check", post(handlers::miauth::miauth_check))
        // AT Protocol XRPC エンドポイント
        .route("/xrpc/com.atproto.server.describeServer", get(handlers::xrpc::server::xrpc_describe_server))
        .route("/xrpc/com.atproto.identity.resolveHandle", get(handlers::xrpc::server::xrpc_resolve_handle))
        .route("/xrpc/com.atproto.sync.getRepo", get(handlers::xrpc::sync::xrpc_get_repo))
        .route("/xrpc/com.atproto.sync.getBlob", get(handlers::xrpc::sync::xrpc_get_blob))
        .route("/xrpc/com.atproto.sync.subscribeRepos", get(handlers::xrpc::sync::xrpc_subscribe_repos))
        .route("/xrpc/com.atproto.repo.getRecord", get(handlers::xrpc::repo::xrpc_get_record))
        // Bsky公式動画パイプライン（uploadVideo）が完了後に呼び戻してくるコールバック
        .route("/xrpc/com.atproto.repo.uploadBlob", post(handlers::xrpc::repo::xrpc_upload_blob))
        // AT Protocol DID 解決
        .route("/.well-known/did.json", get(handlers::xrpc::server::well_known_did))
        .route("/.well-known/atproto-did", get(handlers::xrpc::server::well_known_atproto_did))
        .with_state(state)
        // Misskey クライアントの `i`（ボディ/クエリ）トークンを Authorization ヘッダーへ
        // 合成するブリッジ。既存ハンドラの extract_auth 呼び出しは無改修のまま両対応になる。
        .layer(axum::middleware::from_fn(middleware::misskey_auth_bridge::bridge))
        .layer(cors)
}

/// 起動時タスク: 全ローカルユーザーの Cloudflare TXT 再登録 → Relay requestCrawl →
/// #identity イベントのバックフィル、をこの順でバックグラウンド実行する。
pub fn spawn_startup_tasks(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        ensure_handle_txt_records(&state).await;
        request_relay_crawl(&state).await;
        // requestCrawl 後、Relay が subscribeRepos に接続するまで待機してから
        // #identity をブロードキャストする。
        tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
        backfill_identity_events(&state).await;
    });
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
        let handle = format!("{}.{}", username, state.local_domain);
        match cf.ensure_atproto_txt(&handle, &did).await {
            Ok(_) => tracing::info!("[startup] TXT 確認済み: _atproto.{}", handle),
            Err(e) => tracing::error!("[startup] TXT 登録失敗: {}: {}", handle, e),
        }
    }
}

/// Relay に requestCrawl を送って subscribeRepos 再接続を促す。
async fn request_relay_crawl(state: &AppState) {
    match state
        .http_client
        .post("https://bsky.network/xrpc/com.atproto.sync.requestCrawl")
        .json(&serde_json::json!({"hostname": state.local_domain}))
        .send()
        .await
    {
        Ok(res) => tracing::info!("[atp] 起動時 requestCrawl → {}", res.status()),
        Err(e) => tracing::error!("[atp] 起動時 requestCrawl 失敗: {}", e),
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
        let handle = format!("{}.{}", username, state.local_domain);
        match state.atp_service.broadcast_identity_event(actor_id, &did, &handle, now).await {
            Ok(_) => tracing::info!("[startup] #identity broadcast: {}", handle),
            Err(e) => tracing::error!("[startup] #identity 失敗 {}: {}", handle, e),
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
                    row.id, row.storage_provider_id
                );
            }
            Err(e) => {
                tracing::error!("[media-gc] プロバイダー取得失敗: {}", e);
            }
        }
    }
}
