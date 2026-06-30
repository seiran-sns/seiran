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
use seiran_common::{get_db_pool, SecretsFile};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::Arc;

struct AppState {
    db: PgPool,
    job_queue: Arc<dyn JobQueue>,
    local_domain: String,
    ap_public_key_pem: String,
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

    let state = Arc::new(AppState {
        db,
        job_queue,
        local_domain,
        ap_public_key_pem,
    });

    let app = Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route("/.well-known/nodeinfo", get(nodeinfo_discovery_handler))
        .route("/nodeinfo/2.1", get(nodeinfo_handler))
        .route("/inbox", post(inbox_handler))
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
// POST /inbox — AP アクティビティ受信
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
        None => return (StatusCode::BAD_REQUEST, "resource パラメータが必要です").into_response(),
    };

    let acct = resource.trim_start_matches("acct:");
    let parts: Vec<&str> = acct.splitn(2, '@').collect();
    if parts.len() != 2 {
        return (StatusCode::BAD_REQUEST, "resource フォーマット不正").into_response();
    }
    let (username, domain) = (parts[0], parts[1]);

    if domain != state.local_domain {
        return (StatusCode::NOT_FOUND, "このドメインは管理対象外です").into_response();
    }

    let exists = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    match exists {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[WebFinger] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
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

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/jrd+json")],
        Json(response),
    )
        .into_response()
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
    name: String,
    inbox: String,
    outbox: String,
    followers: String,
    following: String,
    url: String,
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
    let row = sqlx::query(
        "SELECT username, display_name FROM actors
         WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    let display_name = match row {
        Ok(Some(r)) => r
            .try_get::<Option<String>, _>("display_name")
            .ok()
            .flatten()
            .unwrap_or_else(|| username.clone()),
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            eprintln!("[Actor] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let base = format!("https://{}", state.local_domain);
    let actor_uri = format!("{}/users/{}", base, username);

    let doc = ApActorDocument {
        context: vec![
            "https://www.w3.org/ns/activitystreams".to_string(),
            "https://w3id.org/security/v1".to_string(),
        ],
        id: actor_uri.clone(),
        actor_type: "Person".to_string(),
        preferred_username: username.clone(),
        name: display_name,
        inbox: format!("{}/inbox", base),
        outbox: format!("{}/users/{}/outbox", base, username),
        followers: format!("{}/users/{}/followers", base, username),
        following: format!("{}/users/{}/following", base, username),
        url: format!("{}/@{}", base, username),
        public_key: ApPublicKey {
            id: format!("{}#main-key", actor_uri),
            owner: actor_uri,
            public_key_pem: state.ap_public_key_pem.clone(),
        },
    };

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/activity+json")],
        Json(doc),
    )
        .into_response()
}

// =====================================================================
// GET /.well-known/nodeinfo — NodeInfo ディスカバリー
// GET /nodeinfo/2.1       — NodeInfo 本体
// =====================================================================

async fn nodeinfo_discovery_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let body = serde_json::json!({
        "links": [{
            "rel": "http://nodeinfo.diaspora.software/ns/schema/2.1",
            "href": format!("https://{}/nodeinfo/2.1", state.local_domain)
        }]
    });
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Json(body),
    )
        .into_response()
}

async fn nodeinfo_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let user_count: i64 = sqlx::query(
        "SELECT COUNT(*) AS cnt FROM actors WHERE actor_type = 'local'",
    )
    .fetch_one(&state.db)
    .await
    .and_then(|r| r.try_get("cnt"))
    .unwrap_or(0);

    let post_count: i64 = sqlx::query(
        "SELECT COUNT(*) AS cnt FROM posts
         WHERE actor_id IN (SELECT id FROM actors WHERE actor_type = 'local')
           AND deleted_at IS NULL",
    )
    .fetch_one(&state.db)
    .await
    .and_then(|r| r.try_get("cnt"))
    .unwrap_or(0);

    let body = serde_json::json!({
        "version": "2.1",
        "software": {
            "name": "seiran",
            "version": "0.1.0"
        },
        "protocols": ["activitypub"],
        "usage": {
            "users": {
                "total": user_count,
                "activeMonth": user_count,
                "activeHalfyear": user_count
            },
            "localPosts": post_count
        },
        "openRegistrations": true
    });

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json; profile=\"http://nodeinfo.diaspora.software/ns/schema/2.1#\"")],
        Json(body),
    )
        .into_response()
}
