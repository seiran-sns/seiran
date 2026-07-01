mod cloudflare;
mod error;
mod mailer;
mod middleware;
mod handlers;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::{Any, CorsLayer};
use axum::{routing::{get, post}, Router};
use sqlx::PgPool;

use seiran_common::{
    get_db_pool, run_migrations, LocalAuthProvider, Secrets, SecretsFile,
    AtpCommitService, AtpCommitEvent, ApClient,
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
}

// =====================================================================
// エントリーポイント
// =====================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let secrets_file = SecretsFile::from_env();
    let secrets = secrets_file.load_or_create()?;
    eprintln!("[seiran-api] シークレット読み込み完了");

    let pool = get_db_pool().await?;
    eprintln!("[seiran-api] DB 接続完了");
    run_migrations(&pool).await?;
    eprintln!("[seiran-api] マイグレーション適用完了");

    let local_auth = Arc::new(LocalAuthProvider::new(secrets.jwt_secret_bytes()));
    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());
    let http_client = Arc::new(reqwest::Client::new());
    let ap_client = Arc::new(ApClient::new(Arc::clone(&http_client)));
    let crawl_http = Arc::clone(&http_client);
    let crawl_domain = local_domain.clone();
    let startup_domain = local_domain.clone();

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

    // リポジトリ層の構築
    let actors: Arc<dyn ActorRepository> = Arc::new(PgActorRepository::new(pool.clone()));
    let users: Arc<dyn UserRepository> = Arc::new(PgUserRepository::new(pool.clone()));
    let posts: Arc<dyn PostRepository> = Arc::new(PgPostRepository::new(pool.clone()));
    let follows: Arc<dyn FollowRepository> = Arc::new(PgFollowRepository::new(pool.clone()));
    let atp_repo: Arc<dyn AtpReadRepository> = Arc::new(PgAtpReadRepository::new(pool.clone()));

    let startup_actors = Arc::clone(&actors);
    let startup_cf = cloudflare.clone();

    let state = AppState {
        actors,
        users,
        posts,
        follows,
        atp_repo,
        db: pool,
        local_auth,
        miauth_sessions: Arc::new(RwLock::new(HashMap::new())),
        local_domain,
        secrets: Arc::new(secrets),
        atp_service,
        http_client,
        ap_client,
        cloudflare,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
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
        .layer(cors);

    // 起動時タスク: Cloudflare TXT 再登録 + Relay requestCrawl
    {
        let startup_actors = startup_actors;
        let cf2 = startup_cf;
        let hc = crawl_http;
        let domain = crawl_domain;
        let sd = startup_domain;
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

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("[seiran-api] 起動: http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}
