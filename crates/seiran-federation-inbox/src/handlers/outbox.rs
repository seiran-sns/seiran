use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use seiran_common::ap::plain_to_html;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct OutboxQuery {
    page: Option<String>,
    max_id: Option<String>,
}

pub async fn outbox_handler(
    Path(username): Path<String>,
    Query(query): Query<OutboxQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // アクターの存在確認と投稿数取得
    let actor_row = sqlx::query(
        "SELECT a.id, COUNT(p.id) AS total
         FROM actors a
         LEFT JOIN posts p ON p.actor_id = a.id AND p.deleted_at IS NULL
         WHERE a.username = $1 AND a.domain = $2 AND a.actor_type = 'local'
         GROUP BY a.id
         LIMIT 1",
    )
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    let (actor_id, total_items): (i64, i64) = match actor_row {
        Ok(Some(r)) => (
            r.try_get("id").unwrap_or(0),
            r.try_get("total").unwrap_or(0),
        ),
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            eprintln!("[Outbox] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let base = format!("https://{}", state.local_domain);
    let outbox_uri = format!("{}/users/{}/outbox", base, username);
    let actor_uri = format!("{}/users/{}", base, username);
    let followers_uri = format!("{}/followers", actor_uri);
    let actor_key_uri = format!("{}#main-key", actor_uri);
    let _ = actor_key_uri; // Outbox items には publicKey 不要

    // ?page 無し → OrderedCollection（インデックスのみ）
    if query.page.as_deref() != Some("true") {
        let body = serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "OrderedCollection",
            "id": outbox_uri,
            "totalItems": total_items,
            "first": format!("{}?page=true", outbox_uri),
            "last": format!("{}?min_id=0&page=true", outbox_uri)
        });
        return (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/activity+json")],
            Json(body),
        )
            .into_response();
    }

    // ?page=true → OrderedCollectionPage（最大 20 件）
    const PAGE_SIZE: i64 = 20;
    let max_id: Option<i64> = query.max_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match max_id {
        Some(mid) => sqlx::query(
            "SELECT id, body, created_at FROM posts
             WHERE actor_id = $1 AND deleted_at IS NULL AND id < $2
             ORDER BY id DESC LIMIT $3",
        )
        .bind(actor_id)
        .bind(mid)
        .bind(PAGE_SIZE)
        .fetch_all(&state.db)
        .await,
        None => sqlx::query(
            "SELECT id, body, created_at FROM posts
             WHERE actor_id = $1 AND deleted_at IS NULL
             ORDER BY id DESC LIMIT $2",
        )
        .bind(actor_id)
        .bind(PAGE_SIZE)
        .fetch_all(&state.db)
        .await,
    };

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[Outbox] 投稿取得エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let mut ordered_items = Vec::new();
    let mut oldest_id: Option<i64> = None;

    for row in &rows {
        let post_id: i64 = match row.try_get("id") {
            Ok(v) => v,
            Err(_) => continue,
        };
        let body: String = match row.try_get("body") {
            Ok(v) => v,
            Err(_) => continue,
        };
        let created_at: chrono::DateTime<chrono::Utc> = match row.try_get("created_at") {
            Ok(v) => v,
            Err(_) => continue,
        };

        oldest_id = Some(post_id);
        let note_id = format!("{}/notes/{}", base, post_id);
        let activity_id = format!("{}/activities/{}", base, post_id);
        let published = created_at.to_rfc3339();
        let content_html = plain_to_html(&body);

        ordered_items.push(serde_json::json!({
            "type": "Create",
            "id": activity_id,
            "actor": actor_uri,
            "published": published,
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [followers_uri],
            "object": {
                "type": "Note",
                "id": note_id,
                "attributedTo": actor_uri,
                "content": content_html,
                "published": published,
                "to": ["https://www.w3.org/ns/activitystreams#Public"],
                "cc": [followers_uri],
                "url": note_id
            }
        }));
    }

    let mut page = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "OrderedCollectionPage",
        "id": format!("{}?page=true", outbox_uri),
        "partOf": outbox_uri,
        "orderedItems": ordered_items
    });

    // 次ページリンク（取得件数が上限に達した場合）
    if rows.len() as i64 == PAGE_SIZE {
        if let Some(oid) = oldest_id {
            page["next"] = serde_json::json!(
                format!("{}?page=true&max_id={}", outbox_uri, oid)
            );
        }
    }

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/activity+json")],
        Json(page),
    )
        .into_response()
}
