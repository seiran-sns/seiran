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
         WHERE is_local = true AND deleted_at IS NULL",
    )
    .fetch_one(&state.db)
    .await
    .and_then(|r| r.try_get("cnt"))
    .unwrap_or(0);

    // サイト外観（#30/#42）を metadata として同梱する。
    // Misskey 系 nodeinfo の慣習に合わせ nodeName / themeColor / iconUrl を返す。
    let mut appearance: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Ok(rows) = sqlx::query(
        "SELECT key, value FROM site_settings
         WHERE key IN ('site_name', 'site_color', 'site_icon_url')",
    )
    .fetch_all(&state.db)
    .await
    {
        for row in rows {
            if let (Ok(k), Ok(v)) = (row.try_get::<String, _>("key"), row.try_get::<String, _>("value")) {
                appearance.insert(k, v);
            }
        }
    }
    let get = |k: &str| appearance.get(k).cloned().unwrap_or_default();
    let site_name = {
        let n = get("site_name");
        if n.is_empty() { "seiran".to_string() } else { n }
    };
    let site_color = get("site_color");
    let site_icon_url = get("site_icon_url");

    let mut metadata = serde_json::Map::new();
    metadata.insert("nodeName".into(), serde_json::json!(site_name));
    if !site_color.is_empty() {
        metadata.insert("themeColor".into(), serde_json::json!(site_color));
    }
    if !site_icon_url.is_empty() {
        metadata.insert("iconUrl".into(), serde_json::json!(site_icon_url));
    }

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
        "openRegistrations": true,
        "metadata": metadata
    });

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json; profile=\"http://nodeinfo.diaspora.software/ns/schema/2.1#\"")],
        Json(body),
    )
        .into_response()
}
