use axum::{
    routing::{get, post},
    Router,
};
use seiran_common::queue::create_job_queue;
use seiran_common::traits::JobQueue;
use seiran_common::{get_db_pool, SecretsFile};
use sqlx::PgPool;
use std::sync::Arc;

mod error;
mod handlers;

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
    pub local_domain: String,
    pub ap_public_key_pem: String,
    pub ap_private_key_pem: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());

    let db = get_db_pool().await?;
    let job_queue = create_job_queue();

    let secrets_file = SecretsFile::from_env();
    let secrets = secrets_file.load_or_create()?;
    let ap_public_key_pem = secrets.ap_public_key_pem.unwrap_or_default();
    let ap_private_key_pem = secrets.ap_private_key_pem.unwrap_or_default();

    let state = Arc::new(AppState {
        db,
        job_queue,
        local_domain,
        ap_public_key_pem,
        ap_private_key_pem,
    });

    let app = Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route("/.well-known/nodeinfo", get(nodeinfo_discovery_handler))
        .route("/nodeinfo/2.1", get(nodeinfo_handler))
        .route("/inbox", post(inbox_handler))
        .route("/users/:username", get(actor_handler))
        .route("/users/:username/outbox", get(outbox_handler))
        .with_state(state);

    let port = std::env::var("FEDERATION_INBOX_PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("[seiran-federation-inbox] 起動: http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}
