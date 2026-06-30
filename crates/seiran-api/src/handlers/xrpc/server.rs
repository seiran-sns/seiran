use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use sqlx::Row;

use crate::AppState;

#[derive(Deserialize)]
pub struct ResolveHandleQuery {
    pub handle: String,
}

pub async fn xrpc_describe_server(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "did": format!("did:web:{}", state.local_domain),
        "availableUserDomains": [state.local_domain],
        "inviteCodeRequired": false,
        "phoneVerificationRequired": false,
    }))
}

pub async fn xrpc_resolve_handle(
    Query(params): Query<ResolveHandleQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let handle = params.handle.trim().to_lowercase();

    // {username}.{local_domain} 形式かチェック
    let suffix = format!(".{}", state.local_domain);
    let username = if let Some(u) = handle.strip_suffix(&suffix) {
        u.to_string()
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "InvalidRequest", "message": "このPDSが管理していないハンドルです"})),
        ).into_response();
    };

    if username.is_empty() || username.contains('.') {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "InvalidRequest", "message": "無効なハンドルです"})),
        ).into_response();
    }

    let row = sqlx::query(
        "SELECT at_did FROM actors WHERE username = $1 AND domain = $2 AND at_did IS NOT NULL LIMIT 1",
    )
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => match r.try_get::<String, _>("at_did") {
            Ok(did) if !did.is_empty() => Json(serde_json::json!({"did": did})).into_response(),
            _ => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "NotFound", "message": "ハンドルが見つかりません"})),
            ).into_response(),
        },
        _ => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "NotFound", "message": "ハンドルが見つかりません"})),
        ).into_response(),
    }
}

pub async fn well_known_did(State(state): State<AppState>) -> impl IntoResponse {
    let did = format!("did:web:{}", state.local_domain);
    let endpoint = format!("https://{}", state.local_domain);
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Json(serde_json::json!({
            "@context": ["https://www.w3.org/ns/did/v1"],
            "id": did,
            "service": [
                {
                    "id": "#atproto_pds",
                    "type": "AtprotoPersonalDataServer",
                    "serviceEndpoint": endpoint,
                }
            ]
        })),
    )
}

pub async fn well_known_atproto_did(
    axum::extract::Host(host): axum::extract::Host,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let username = host.split('.').next().unwrap_or("").to_string();

    if username.is_empty() || username == state.local_domain {
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    let row = sqlx::query(
        "SELECT at_did FROM actors WHERE username = $1 AND domain = $2 AND at_did IS NOT NULL LIMIT 1",
    )
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => {
            let did: String = match r.try_get("at_did") {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[well_known_atproto_did] at_did 取得失敗: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
                }
            };
            if did.is_empty() {
                (StatusCode::NOT_FOUND, "").into_response()
            } else {
                ([(axum::http::header::CONTENT_TYPE, "text/plain")], did).into_response()
            }
        }
        _ => (StatusCode::NOT_FOUND, "").into_response(),
    }
}
