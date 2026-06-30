use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

pub async fn nodeinfo_discovery_handler(
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

pub async fn nodeinfo_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let user_count: i64 =
        sqlx::query("SELECT COUNT(*) AS cnt FROM actors WHERE actor_type = 'local'")
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
