use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use seiran_common::ap::deliver::at_uri_to_bsky_app_url;
use seiran_common::ap::plain_to_html;
use serde::Deserialize;
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
         WHERE a.username = $1 AND a.actor_type = 'local'
         GROUP BY a.id
         LIMIT 1",
    )
    .bind(&username)
    .fetch_optional(&state.db)
    .await;

    let (actor_id, total_items): (i64, i64) = match actor_row {
        Ok(Some(r)) => (
            r.try_get("id").unwrap_or(0),
            r.try_get("total").unwrap_or(0),
        ),
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::error!("[Outbox] DB エラー: {}", e);
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
            [(
                axum::http::header::CONTENT_TYPE,
                "application/activity+json",
            )],
            Json(body),
        )
            .into_response();
    }

    // ?page=true → OrderedCollectionPage（最大 20 件）
    const PAGE_SIZE: i64 = 20;
    let max_id: Option<i64> = query.max_id.as_deref().and_then(|s| s.parse().ok());

    // リポスト行は本文（body）が常に空文字列で、単独では Create(Note) として
    // 表現できない（元投稿を参照する Announce として表現する必要がある）ため、
    // リポスト元（orig）の ap_object_id / at_uri / 投稿者情報も合わせて取得する。
    const SELECT_COLUMNS: &str = "p.id, p.body, p.created_at, p.repost_of_post_id, p.ap_object_id,
             orig.ap_object_id AS orig_ap_object_id, orig.at_uri AS orig_at_uri,
             oa.username AS orig_username, oa.display_name AS orig_display_name,
             oa.ap_uri AS orig_actor_uri";
    let rows = match max_id {
        Some(mid) => {
            sqlx::query(&format!(
                "SELECT {SELECT_COLUMNS} FROM posts p
             LEFT JOIN posts orig ON orig.id = p.repost_of_post_id
             LEFT JOIN actors oa ON oa.id = orig.actor_id
             WHERE p.actor_id = $1 AND p.deleted_at IS NULL AND p.id < $2
             ORDER BY p.id DESC LIMIT $3"
            ))
            .bind(actor_id)
            .bind(mid)
            .bind(PAGE_SIZE)
            .fetch_all(&state.db)
            .await
        }
        None => {
            sqlx::query(&format!(
                "SELECT {SELECT_COLUMNS} FROM posts p
             LEFT JOIN posts orig ON orig.id = p.repost_of_post_id
             LEFT JOIN actors oa ON oa.id = orig.actor_id
             WHERE p.actor_id = $1 AND p.deleted_at IS NULL
             ORDER BY p.id DESC LIMIT $2"
            ))
            .bind(actor_id)
            .bind(PAGE_SIZE)
            .fetch_all(&state.db)
            .await
        }
    };

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[Outbox] 投稿取得エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // 取得した post_id のリストで添付ファイルをまとめて取得
    let post_ids: Vec<i64> = rows.iter().filter_map(|r| r.try_get("id").ok()).collect();
    let mut att_map: std::collections::HashMap<i64, Vec<serde_json::Value>> =
        std::collections::HashMap::new();
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
            let pid: i64 = match r.try_get("post_id") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let storage_key: String = match r.try_get("storage_key") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mime_type: String = match r.try_get("mime_type") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let width: i32 = match r.try_get("width") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let height: i32 = match r.try_get("height") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let public_url: String = match r.try_get("public_url") {
                Ok(v) => v,
                Err(_) => continue,
            };
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

    let mut ordered_items = Vec::new();
    let mut oldest_id: Option<i64> = None;

    for row in &rows {
        let post_id: i64 = match row.try_get("id") {
            Ok(v) => v,
            Err(_) => continue,
        };
        let created_at: chrono::DateTime<chrono::Utc> = match row.try_get("created_at") {
            Ok(v) => v,
            Err(_) => continue,
        };
        let own_ap_object_id: Option<String> = row.try_get("ap_object_id").unwrap_or(None);
        let repost_of_post_id: Option<i64> = row.try_get("repost_of_post_id").unwrap_or(None);
        let published = created_at.to_rfc3339();

        oldest_id = Some(post_id);

        // リポスト行: body は常に空文字列のため、push 配送（deliver_ap_announce /
        // deliver_post_to_ap_followers）と同じ表現で Announce または Create(Note)
        // として組み立てる。素通しで body を Create(Note) 化すると、push 側とは
        // 別の AP object id を持つ「空の通常ポスト」がリモートに二重出現する。
        if repost_of_post_id.is_some() {
            let orig_ap_object_id: Option<String> =
                row.try_get("orig_ap_object_id").unwrap_or(None);
            let Some(own_id) = own_ap_object_id else {
                continue;
            };

            if let Some(orig_id) = orig_ap_object_id {
                let mut cc = vec![followers_uri.clone()];
                if let Some(orig_actor_uri) = row
                    .try_get::<Option<String>, _>("orig_actor_uri")
                    .unwrap_or(None)
                {
                    cc.push(orig_actor_uri);
                }
                ordered_items.push(serde_json::json!({
                    "type": "Announce",
                    "id": own_id,
                    "actor": actor_uri,
                    "published": published,
                    "to": ["https://www.w3.org/ns/activitystreams#Public"],
                    "cc": cc,
                    "object": orig_id
                }));
            } else if let Some(orig_at_uri) = row
                .try_get::<Option<String>, _>("orig_at_uri")
                .unwrap_or(None)
            {
                // Bsky ネイティブ投稿のリポスト → Fedi フォールバック（テキスト投稿）。
                // push 配送側（deliver_repost）と同じ本文を組み立てる。
                let orig_username: String = row.try_get("orig_username").unwrap_or_default();
                let orig_display_name: Option<String> =
                    row.try_get("orig_display_name").unwrap_or(None);
                let author_name = orig_display_name.as_deref().unwrap_or(&orig_username);
                let bsky_url = at_uri_to_bsky_app_url(&orig_at_uri);
                let content_html = plain_to_html(&format!("🔁 {}: {}", author_name, bsky_url));
                let activity_id = format!("{}/activities/{}", base, post_id);
                let note_obj = serde_json::json!({
                    "type": "Note",
                    "id": own_id,
                    "attributedTo": actor_uri,
                    "content": content_html,
                    "published": published,
                    "to": ["https://www.w3.org/ns/activitystreams#Public"],
                    "cc": [followers_uri],
                    "url": own_id
                });
                ordered_items.push(serde_json::json!({
                    "type": "Create",
                    "id": activity_id,
                    "actor": actor_uri,
                    "published": published,
                    "to": ["https://www.w3.org/ns/activitystreams#Public"],
                    "cc": [followers_uri],
                    "object": note_obj
                }));
            }
            // リポスト元がどちらの ID も持たない（削除済み等）場合は表現不能のためスキップ。
            continue;
        }

        let body: String = match row.try_get("body") {
            Ok(v) => v,
            Err(_) => continue,
        };
        let note_id = own_ap_object_id.unwrap_or_else(|| format!("{}/notes/{}", base, post_id));
        let activity_id = format!("{}/activities/{}", base, post_id);
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

        ordered_items.push(serde_json::json!({
            "type": "Create",
            "id": activity_id,
            "actor": actor_uri,
            "published": published,
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [followers_uri],
            "object": note_obj
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
            page["next"] = serde_json::json!(format!("{}?page=true&max_id={}", outbox_uri, oid));
        }
    }

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/activity+json",
        )],
        Json(page),
    )
        .into_response()
}
