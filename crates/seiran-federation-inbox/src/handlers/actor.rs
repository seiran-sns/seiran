use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    inbox: String,
    outbox: String,
    followers: String,
    following: String,
    /// ピン留め投稿（#61）。Mastodon 等はプロフィール表示時にこの URL を都度フェッチする。
    featured: String,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<ApImage>,
    #[serde(rename = "publicKey")]
    public_key: ApPublicKey,
}

#[derive(Serialize)]
struct ApImage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "mediaType")]
    media_type: String,
    url: String,
}

#[derive(Serialize)]
struct ApPublicKey {
    id: String,
    owner: String,
    #[serde(rename = "publicKeyPem")]
    public_key_pem: String,
}

pub async fn actor_handler(
    Path(username): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let row = sqlx::query(
        "SELECT a.display_name, a.bio, \
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url, \
                mf.mime_type AS avatar_mime_type \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.username = $1 AND a.domain = $2 AND a.actor_type = 'local' LIMIT 1",
    )
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    let (display_name, bio, avatar_url, avatar_mime_type) = match row {
        Ok(Some(r)) => {
            let display_name = r
                .try_get::<Option<String>, _>("display_name")
                .ok()
                .flatten()
                .unwrap_or_else(|| username.clone());
            let bio = r.try_get::<Option<String>, _>("bio").ok().flatten();
            let avatar_url = r.try_get::<Option<String>, _>("avatar_url").ok().flatten();
            let avatar_mime_type = r.try_get::<Option<String>, _>("avatar_mime_type").ok().flatten();
            (display_name, bio, avatar_url, avatar_mime_type)
        }
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::error!("[Actor] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let base = format!("https://{}", state.local_domain);
    let actor_uri = format!("{}/users/{}", base, username);

    let icon = avatar_url.map(|url| ApImage {
        kind: "Image".to_string(),
        media_type: avatar_mime_type.unwrap_or_else(|| "image/jpeg".to_string()),
        url,
    });

    let doc = ApActorDocument {
        context: vec![
            "https://www.w3.org/ns/activitystreams".to_string(),
            "https://w3id.org/security/v1".to_string(),
        ],
        id: actor_uri.clone(),
        actor_type: "Person".to_string(),
        preferred_username: username.clone(),
        name: display_name,
        summary: bio,
        inbox: format!("{}/inbox", base),
        outbox: format!("{}/users/{}/outbox", base, username),
        followers: format!("{}/users/{}/followers", base, username),
        following: format!("{}/users/{}/following", base, username),
        featured: format!("{}/users/{}/collections/featured", base, username),
        url: format!("{}/@{}", base, username),
        icon,
        public_key: ApPublicKey {
            id: format!("{}#main-key", actor_uri),
            owner: actor_uri,
            public_key_pem: state.ap_public_key_pem.clone(),
        },
    };

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/activity+json",
        )],
        Json(doc),
    )
        .into_response()
}
