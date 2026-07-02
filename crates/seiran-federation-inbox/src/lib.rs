//! seiran-federation-inbox — ActivityPub Inbox / WebFinger / Actor / Outbox / NodeInfo。
//!
//! バイナリは `seiran-server` が `--role federation`（または `all`）で起動する。
//! 共有リソースを受け取り AppState を構築（[`init_state`]）し、ルーターを返す（[`router`]）。

pub mod error;
pub mod handlers;

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};
use seiran_common::queue::create_job_queue;
use seiran_common::repository::{
    ActorRepository, FollowRepository, PgActorRepository, PgFollowRepository, PgPostRepository,
    PostRepository,
};
use seiran_common::traits::JobQueue;
use seiran_common::{ApClient, Secrets};
use sqlx::PgPool;

use handlers::{
    actor::actor_handler,
    inbox::inbox_handler,
    nodeinfo::{nodeinfo_discovery_handler, nodeinfo_handler},
    outbox::outbox_handler,
    webfinger::webfinger_handler,
};

pub struct AppState {
    pub db: PgPool,
    pub job_queue: Arc<dyn JobQueue>,
    pub actor_repo: Arc<dyn ActorRepository>,
    pub follow_repo: Arc<dyn FollowRepository>,
    pub post_repo: Arc<dyn PostRepository>,
    pub local_domain: String,
    pub ap_public_key_pem: String,
    pub ap_private_key_pem: String,
    pub ap_client: Arc<ApClient>,
}

/// 共有リソースを受け取り federation ロールの [`AppState`] を構築する。
pub fn init_state(
    pool: PgPool,
    secrets: &Secrets,
    http_client: Arc<reqwest::Client>,
    local_domain: String,
) -> Arc<AppState> {
    let job_queue = create_job_queue();
    let actor_repo: Arc<dyn ActorRepository> = Arc::new(PgActorRepository::new(pool.clone()));
    let follow_repo: Arc<dyn FollowRepository> = Arc::new(PgFollowRepository::new(pool.clone()));
    let post_repo: Arc<dyn PostRepository> = Arc::new(PgPostRepository::new(pool.clone()));
    let ap_public_key_pem = secrets.ap_public_key_pem.clone().unwrap_or_default();
    let ap_private_key_pem = secrets.ap_private_key_pem.clone().unwrap_or_default();
    let ap_client = Arc::new(ApClient::new(http_client));

    Arc::new(AppState {
        db: pool,
        job_queue,
        actor_repo,
        follow_repo,
        post_repo,
        local_domain,
        ap_public_key_pem,
        ap_private_key_pem,
        ap_client,
    })
}

/// federation ロールの axum ルーターを構築する。
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route("/.well-known/nodeinfo", get(nodeinfo_discovery_handler))
        .route("/nodeinfo/2.1", get(nodeinfo_handler))
        .route("/inbox", post(inbox_handler))
        .route("/users/:username", get(actor_handler))
        .route("/users/:username/outbox", get(outbox_handler))
        .with_state(state)
}
