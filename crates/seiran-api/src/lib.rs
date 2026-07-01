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

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::{Any, CorsLayer};
use axum::{routing::{get, patch, post}, Router};
use sqlx::PgPool;

use seiran_common::{
    LocalAuthProvider, Secrets, AtpCommitService, AtpCommitEvent, ApClient,
    StorageProviderRepository, PgStorageProviderRepository,
    MediaFileRepository, PgMediaFileRepository,
};
use seiran_common::repository::{
    ActorRepository, AtpReadRepository, FollowRepository, PostRepository, UserRepository,
    PgActorRepository, PgAtpReadRepository, PgFollowRepository, PgPostRepository, PgUserRepository,
};

use handlers::miauth::MiAuthSession;

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
) -> AppState {
    let local_auth = Arc::new(LocalAuthProvider::new(secrets.jwt_secret_bytes()));
    let ap_client = Arc::new(ApClient::new(Arc::clone(&http_client)));

    let (atp_event_tx, _) = broadcast::channel::<AtpCommitEvent>(1024);
    let atp_event_tx = Arc::new(atp_event_tx);

    let atp_service = Arc::new(AtpCommitService::new(
        pool.clone(),
        Arc::clone(&atp_event_tx),
        Arc::clone(&http_client),
    ));

    let cloudflare = match (
        std::env::var("CLOUDFLARE_API_TOKEN"),
        std::env::var("CLOUDFLARE_ZONE_ID"),
    ) {
        (Ok(token), Ok(zone_id)) if !token.is_empty() && !zone_id.is_empty() => {
            eprintln!("[seiran-api] Cloudflare DNS ハンドル検証: 有効");
            Some(Arc::new(cloudflare::CloudflareClient::new(
                Arc::clone(&http_client),
                token,
                zone_id,
            )))
        }
        _ => {
            eprintln!("[seiran-api] Cloudflare DNS ハンドル検証: 無効 (HTTP well-known のみ)");
            None
        }
    };

    let enc_key = secrets.encryption_key_bytes();
    let storage_providers: Arc<dyn StorageProviderRepository> =
        Arc::new(PgStorageProviderRepository::new(pool.clone(), enc_key));
    let media_files: Arc<dyn MediaFileRepository> =
        Arc::new(PgMediaFileRepository::new(pool.clone()));
    let actors: Arc<dyn ActorRepository> = Arc::new(PgActorRepository::new(pool.clone()));
    let users: Arc<dyn UserRepository> = Arc::new(PgUserRepository::new(pool.clone()));
    let posts: Arc<dyn PostRepository> = Arc::new(PgPostRepository::new(pool.clone()));
    let follows: Arc<dyn FollowRepository> = Arc::new(PgFollowRepository::new(pool.clone()));
    let atp_repo: Arc<dyn AtpReadRepository> = Arc::new(PgAtpReadRepository::new(pool.clone()));

    AppState {
        actors,
        users,
        posts,
        follows,
        atp_repo,
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
    }
}

/// api ロールの axum ルーターを構築する（CORS 適用込み）。
pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
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
        // ドライブ（メディアアップロード）
        .route("/api/drive/files/create", post(handlers::drive::create_drive_file))
        // 認証
        .route("/api/auth/verify-email", post(handlers::email_verify::request_email_verification))
        .route("/api/auth/verify-token", get(handlers::email_verify::verify_email_token))
        .route("/api/auth/register", post(handlers::auth::register))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/me", get(handlers::auth::me))
        // 投稿
        .route("/api/notes/create", post(handlers::notes::create_note))
        .route("/api/notes/local-timeline", get(handlers::notes::local_timeline))
        .route("/api/notes/home-timeline", get(handlers::notes::home_timeline))
        .route("/api/notes/:id", get(handlers::notes::get_note))
        // ActivityPub Note エンドポイント（nginx が AP Accept ヘッダーのみをここへ転送）
        .route("/notes/:id", get(handlers::notes::get_note_ap))
        // フォロー
        .route("/api/follows/create", post(handlers::follows::create_follow))
        // ユーザープロフィール
        .route("/api/users/profile", get(handlers::users::user_profile))
        // Misskey 互換レイヤー
        .route("/api/meta", post(handlers::meta::api_meta))
        // MiAuth（Misskey 互換クライアント用）
        .route("/miauth/:session_id", get(handlers::miauth::miauth_page))
        .route("/miauth/:session_id/authorize", post(handlers::miauth::miauth_authorize))
        .route("/api/miauth/:session_id/check", post(handlers::miauth::miauth_check_by_path))
        .route("/api/miauth/check", post(handlers::miauth::miauth_check))
        // AT Protocol XRPC エンドポイント
        .route("/xrpc/com.atproto.server.describeServer", get(handlers::xrpc::server::xrpc_describe_server))
        .route("/xrpc/com.atproto.identity.resolveHandle", get(handlers::xrpc::server::xrpc_resolve_handle))
        .route("/xrpc/com.atproto.sync.getRepo", get(handlers::xrpc::sync::xrpc_get_repo))
        .route("/xrpc/com.atproto.sync.subscribeRepos", get(handlers::xrpc::sync::xrpc_subscribe_repos))
        .route("/xrpc/com.atproto.repo.getRecord", get(handlers::xrpc::repo::xrpc_get_record))
        // AT Protocol DID 解決
        .route("/.well-known/did.json", get(handlers::xrpc::server::well_known_did))
        .route("/.well-known/atproto-did", get(handlers::xrpc::server::well_known_atproto_did))
        .with_state(state)
        .layer(cors)
}

/// 起動時タスク: 全ローカルユーザーの Cloudflare TXT 再登録 + Relay requestCrawl。
///
/// 再デプロイ後の TXT 消失対策と subscribeRepos 再接続の促進のため、
/// バックグラウンドで一度だけ実行する。
pub fn spawn_startup_tasks(state: &AppState) {
    let startup_actors = Arc::clone(&state.actors);
    let cf2 = state.cloudflare.clone();
    let hc = Arc::clone(&state.http_client);
    let domain = state.local_domain.clone();
    let sd = state.local_domain.clone();

    tokio::spawn(async move {
        // 全ローカルユーザーのハンドル TXT を確保（再デプロイ後の消失対策）
        if let Some(cf) = cf2 {
            match startup_actors.list_local_dids().await {
                Ok(rows) => {
                    for (username, did) in rows {
                        let handle = format!("{}.{}", username, sd);
                        match cf.ensure_atproto_txt(&handle, &did).await {
                            Ok(_) => eprintln!("[startup] TXT 確認済み: _atproto.{}", handle),
                            Err(e) => eprintln!("[startup] TXT 登録失敗: {}: {}", handle, e),
                        }
                    }
                }
                Err(e) => eprintln!("[startup] ローカルユーザー取得失敗: {}", e),
            }
        }

        // Relay に requestCrawl を送って subscribeRepos 再接続を促す
        match hc
            .post("https://bsky.network/xrpc/com.atproto.sync.requestCrawl")
            .json(&serde_json::json!({"hostname": domain}))
            .send()
            .await
        {
            Ok(res) => eprintln!("[atp] 起動時 requestCrawl → {}", res.status()),
            Err(e) => eprintln!("[atp] 起動時 requestCrawl 失敗: {}", e),
        }
    });
}
