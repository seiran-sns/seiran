use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use seiran_common::ap::verify_signature;
use seiran_common::queue::create_job_queue;
use seiran_common::traits::{Job, JobQueue};
use std::collections::HashMap;
use std::sync::Arc;

struct AppState {
    job_queue: Arc<dyn JobQueue>,
    local_domain: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let job_queue = create_job_queue();
    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());

    let state = Arc::new(AppState {
        job_queue,
        local_domain,
    });

    let app = Router::new()
        .route("/inbox", post(inbox_handler))
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route("/users/:username", get(actor_handler))
        .with_state(state);

    let port = std::env::var("FEDERATION_INBOX_PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("[seiran-federation-inbox] 起動: http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

// =====================================================================
// POST /inbox — AP アクティビティの受け入れ
// =====================================================================

async fn inbox_handler(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let header_map: HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|val| (k.as_str().to_lowercase(), val.to_string()))
        })
        .collect();

    let signature = match header_map.get("signature") {
        Some(s) => s.clone(),
        None => {
            return (StatusCode::UNAUTHORIZED, "署名ヘッダーが見つかりません").into_response();
        }
    };

    match verify_signature("POST", "/inbox", &header_map, &signature).await {
        Ok(true) => {}
        Ok(false) => {
            return (StatusCode::UNAUTHORIZED, "署名検証失敗").into_response();
        }
        Err(e) => {
            eprintln!("[Inbox] 署名検証エラー: {}", e);
            return (StatusCode::UNAUTHORIZED, format!("署名エラー: {}", e)).into_response();
        }
    }

    let raw_activity = String::from_utf8_lossy(&body).to_string();
    eprintln!("[Inbox] アクティビティ受信 ({} bytes)", raw_activity.len());

    if let Err(e) = state
        .job_queue
        .enqueue(Job::InboundActivityProcess { raw_activity }, 10)
        .await
    {
        eprintln!("[Inbox] エンキュー失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "エンキュー失敗").into_response();
    }

    (StatusCode::ACCEPTED, "").into_response()
}

// =====================================================================
// GET /.well-known/webfinger?resource=acct:username@domain
// =====================================================================

#[derive(Deserialize)]
struct WebFingerQuery {
    resource: Option<String>,
}

#[derive(Serialize)]
struct WebFingerResponse {
    subject: String,
    aliases: Vec<String>,
    links: Vec<WebFingerLink>,
}

#[derive(Serialize)]
struct WebFingerLink {
    rel: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    href: Option<String>,
}

async fn webfinger_handler(
    Query(query): Query<WebFingerQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let resource = match query.resource {
        Some(r) => r,
        None => {
            return (StatusCode::BAD_REQUEST, "resource パラメータが必要です").into_response();
        }
    };

    let acct = resource.trim_start_matches("acct:");
    let parts: Vec<&str> = acct.splitn(2, '@').collect();
    if parts.len() != 2 {
        return (
            StatusCode::BAD_REQUEST,
            "resource フォーマット不正 (例: acct:user@domain.example)",
        )
            .into_response();
    }
    let (username, domain) = (parts[0], parts[1]);

    if domain != state.local_domain {
        return (StatusCode::NOT_FOUND, "このドメインは管理対象外です").into_response();
    }

    let actor_uri = format!("https://{}/users/{}", state.local_domain, username);
    let response = WebFingerResponse {
        subject: format!("acct:{}@{}", username, state.local_domain),
        aliases: vec![actor_uri.clone()],
        links: vec![WebFingerLink {
            rel: "self".to_string(),
            mime_type: Some("application/activity+json".to_string()),
            href: Some(actor_uri),
        }],
    };

    (StatusCode::OK, Json(response)).into_response()
}

// =====================================================================
// GET /users/:username — AP アクタードキュメント
// =====================================================================

#[derive(Serialize)]
struct ApActorDocument {
    #[serde(rename = "@context")]
    context: Vec<String>,
    id: String,
    #[serde(rename = "type")]
    actor_type: String,
    #[serde(rename = "preferredUsername")]
    preferred_username: String,
    inbox: String,
    outbox: String,
    followers: String,
    following: String,
    #[serde(rename = "publicKey")]
    public_key: ApPublicKey,
}

#[derive(Serialize)]
struct ApPublicKey {
    id: String,
    owner: String,
    #[serde(rename = "publicKeyPem")]
    public_key_pem: String,
}

async fn actor_handler(
    Path(username): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let base = format!("https://{}", state.local_domain);
    let actor_uri = format!("{}/users/{}", base, username);

    let secrets_file = seiran_common::SecretsFile::from_env();
    let secrets = match secrets_file.load_or_create() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[Actor] secrets.toml 読み込み失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "設定エラー").into_response();
        }
    };

    let public_key_pem = secrets.ap_public_key_pem.unwrap_or_default();

    let doc = ApActorDocument {
        context: vec![
            "https://www.w3.org/ns/activitystreams".to_string(),
            "https://w3id.org/security/v1".to_string(),
        ],
        id: actor_uri.clone(),
        actor_type: "Person".to_string(),
        preferred_username: username.clone(),
        inbox: format!("{}/inbox", base),
        outbox: format!("{}/users/{}/outbox", base, username),
        followers: format!("{}/users/{}/followers", base, username),
        following: format!("{}/users/{}/following", base, username),
        public_key: ApPublicKey {
            id: format!("{}#main-key", actor_uri),
            owner: actor_uri.clone(),
            public_key_pem,
        },
    };

    (StatusCode::OK, Json(doc)).into_response()
}
