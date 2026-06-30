use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use sqlx::Row;

use crate::AppState;

pub async fn xrpc_describe_server(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "did": format!("did:web:{}", state.local_domain),
        "availableUserDomains": [state.local_domain],
        "inviteCodeRequired": false,
        "phoneVerificationRequired": false,
    }))
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
