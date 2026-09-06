use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct WebFingerQuery {
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

pub async fn webfinger_handler(
    Query(query): Query<WebFingerQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let resource = match query.resource {
        Some(r) => r,
        None => return (StatusCode::BAD_REQUEST, "resource パラメータが必要です").into_response(),
    };

    let username = if let Some(acct) = resource.strip_prefix("acct:") {
        let parts: Vec<&str> = acct.splitn(2, '@').collect();
        if parts.len() != 2 {
            return (StatusCode::BAD_REQUEST, "resource フォーマット不正").into_response();
        }
        let (username, domain) = (parts[0], parts[1]);
        if domain != state.local_domain {
            return (StatusCode::NOT_FOUND, "このドメインは管理対象外です").into_response();
        }
        username.to_string()
    } else {
        // 旧形式プロフィールURL（AP actor ID、`/@handle`表記へ移行する前は
        // これが唯一の外部公開URLだった）を resource に直接渡す再検証リクエストに対応する
        // （リモートが acct: を経ずキャッシュ済み actor URL で WebFinger する場合がある）。
        // `docs/architecture.md` 8.2節参照。
        let prefix = format!("https://{}/users/", state.local_domain);
        match resource.strip_prefix(&prefix) {
            Some(rest) if !rest.is_empty() && !rest.contains('/') => rest.to_string(),
            _ => return (StatusCode::BAD_REQUEST, "resource フォーマット不正").into_response(),
        }
    };
    let username = username.as_str();

    let exists = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND actor_type = 'local' AND withdrawn_at IS NULL LIMIT 1",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await;

    match exists {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
        Err(e) => {
            tracing::error!("[WebFinger] DB エラー: {}", e);
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
        [(
            axum::http::header::CONTENT_TYPE,
            "application/jrd+json; charset=utf-8",
        )],
        Json(response),
    )
        .into_response()
}
