use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use seiran_common::ap::plain_to_html;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

/// GET /users/:username/collections/featured
/// ピン留め投稿（#61）を `OrderedCollection` として返す。Mastodon 等の実装はプロフィール
/// 取得時にこのコレクションを都度フェッチしてピン留め表示を更新する（Add/Remove Activity
/// の配送は行わない、最大5件のためページングも行わない）。
pub async fn featured_handler(
    Path(username): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let actor_row = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    let actor_id: i64 = match actor_row {
        Ok(Some(r)) => r.try_get("id").unwrap_or(0),
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::error!("[Featured] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let base = format!("https://{}", state.local_domain);
    let featured_uri = format!("{}/users/{}/collections/featured", base, username);
    let actor_uri = format!("{}/users/{}", base, username);
    let followers_uri = format!("{}/followers", actor_uri);

    let rows = sqlx::query(
        "SELECT p.id, p.body, p.created_at
         FROM pinned_posts pp
         JOIN posts p ON p.id = pp.post_id
         WHERE pp.actor_id = $1 AND p.deleted_at IS NULL
         ORDER BY pp.pinned_at DESC",
    )
    .bind(actor_id)
    .fetch_all(&state.db)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[Featured] 投稿取得エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // 取得した post_id のリストで添付ファイルをまとめて取得（outbox_handler と同じパターン）
    let post_ids: Vec<i64> = rows.iter().filter_map(|r| r.try_get("id").ok()).collect();
    let mut att_map: std::collections::HashMap<i64, Vec<serde_json::Value>> = std::collections::HashMap::new();
    if !post_ids.is_empty() {
        let att_rows = sqlx::query(
            "SELECT pa.post_id, mf.storage_key, mf.mime_type, mf.width, mf.height, sp.public_url
             FROM post_attachments pa
             JOIN media_files mf ON mf.id = pa.media_file_id
             JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE pa.post_id = ANY($1)
             ORDER BY pa.post_id, pa.position",
        )
        .bind(&post_ids)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for r in &att_rows {
            let pid: i64 = match r.try_get("post_id") { Ok(v) => v, Err(_) => continue };
            let storage_key: String = match r.try_get("storage_key") { Ok(v) => v, Err(_) => continue };
            let mime_type: String = match r.try_get("mime_type") { Ok(v) => v, Err(_) => continue };
            let width: i32 = match r.try_get("width") { Ok(v) => v, Err(_) => continue };
            let height: i32 = match r.try_get("height") { Ok(v) => v, Err(_) => continue };
            let public_url: String = match r.try_get("public_url") { Ok(v) => v, Err(_) => continue };
            let url = format!("{}/{}", public_url.trim_end_matches('/'), storage_key);
            att_map.entry(pid).or_default().push(serde_json::json!({
                "type": "Document",
                "mediaType": mime_type,
                "url": url,
                "width": width,
                "height": height
            }));
        }
    }

    // featured collection は Note オブジェクトを直接（Create でラップせずに）並べる
    // （Mastodon 等の実装と同じ慣習）。
    let mut ordered_items = Vec::new();
    for row in &rows {
        let post_id: i64 = match row.try_get("id") { Ok(v) => v, Err(_) => continue };
        let body: String = match row.try_get("body") { Ok(v) => v, Err(_) => continue };
        let created_at: chrono::DateTime<chrono::Utc> = match row.try_get("created_at") { Ok(v) => v, Err(_) => continue };

        let note_id = format!("{}/notes/{}", base, post_id);
        let published = created_at.to_rfc3339();
        let content_html = plain_to_html(&body);
        let attachments = att_map.remove(&post_id).unwrap_or_default();

        let mut note_obj = serde_json::json!({
            "type": "Note",
            "id": note_id,
            "attributedTo": actor_uri,
            "content": content_html,
            "published": published,
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [followers_uri],
            "url": note_id
        });
        if !attachments.is_empty() {
            note_obj["attachment"] = serde_json::Value::Array(attachments);
        }
        ordered_items.push(note_obj);
    }

    let body = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "OrderedCollection",
        "id": featured_uri,
        "totalItems": ordered_items.len(),
        "orderedItems": ordered_items
    });

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/activity+json")],
        Json(body),
    )
        .into_response()
}
