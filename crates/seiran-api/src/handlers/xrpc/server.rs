use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;

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

    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "NotFound", "message": "ハンドルが見つかりません"})),
        )
            .into_response()
    };

    match state
        .actors
        .find_did_by_username_domain(&username, &state.local_domain)
        .await
    {
        Ok(Some(did)) if !did.is_empty() => {
            Json(serde_json::json!({"did": did})).into_response()
        }
        Ok(_) => not_found(),
        Err(e) => {
            eprintln!("[resolveHandle] DB エラー: {}", e);
            not_found()
        }
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

    match state
        .actors
        .find_did_by_username_domain(&username, &state.local_domain)
        .await
    {
        Ok(Some(did)) if !did.is_empty() => {
            ([(axum::http::header::CONTENT_TYPE, "text/plain")], did).into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            eprintln!("[well_known_atproto_did] DB エラー: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "").into_response()
        }
    }
}
