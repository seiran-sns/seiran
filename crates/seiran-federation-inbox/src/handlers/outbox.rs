use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use seiran_common::repository::ap_public::OutboxRow;
use serde::Deserialize;
use std::sync::Arc;

use super::ap_collection::{activity_json, local_note, AS_PUBLIC};
use crate::AppState;

#[derive(Deserialize)]
pub struct OutboxQuery {
    page: Option<String>,
    max_id: Option<String>,
}

/// 1ページ（`?page=true`）あたりの最大件数。
const PAGE_SIZE: i64 = 20;

pub async fn outbox_handler(
    Path(username): Path<String>,
    Query(query): Query<OutboxQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (actor_id, total_items) =
        match seiran_common::repository::ap_public::outbox_owner(&state.db, &username).await {
            Ok(Some(v)) => v,
            Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
            Err(e) => {
                tracing::error!("[Outbox] DB エラー: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
            }
        };

    let base = format!("https://{}", state.local_domain);
    let outbox_uri = format!("{}/users/{}/outbox", base, username);

    // ?page 無し → OrderedCollection（インデックスのみ）
    if query.page.as_deref() != Some("true") {
        return activity_json(serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "OrderedCollection",
            "id": outbox_uri,
            "totalItems": total_items,
            "first": format!("{}?page=true", outbox_uri),
            "last": format!("{}?min_id=0&page=true", outbox_uri)
        }));
    }

    // ?page=true → OrderedCollectionPage（最大 PAGE_SIZE 件）
    let max_id: Option<i64> = query.max_id.as_deref().and_then(|s| s.parse().ok());
    let rows = match seiran_common::repository::ap_public::outbox_page(
        &state.db, actor_id, max_id, PAGE_SIZE,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[Outbox] 投稿取得エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let actor_uri = format!("{}/users/{}", base, username);
    let followers_uri = format!("{}/followers", actor_uri);
    let mut ordered_items: Vec<serde_json::Value> = Vec::with_capacity(rows.len());
    for row in &rows {
        if let Some(item) = outbox_item(&state, actor_id, &actor_uri, &followers_uri, row).await {
            ordered_items.push(item);
        }
    }

    let mut page = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "OrderedCollectionPage",
        "id": format!("{}?page=true", outbox_uri),
        "partOf": outbox_uri,
        "orderedItems": ordered_items
    });

    // 次ページリンク（取得件数が上限に達した場合）
    if let Some(oldest) = rows.last().filter(|_| rows.len() as i64 == PAGE_SIZE) {
        page["next"] = serde_json::json!(format!("{}?page=true&max_id={}", outbox_uri, oldest.id));
    }

    activity_json(page)
}

/// 投稿1件を outbox の項目（Create または Announce）に変換する。表現できない行は `None`。
/// Create の Note は配送・`GET /notes/:id` と同じ`build_local_note`で組み立てる。
async fn outbox_item(
    state: &AppState,
    actor_id: i64,
    actor_uri: &str,
    followers_uri: &str,
    row: &OutboxRow,
) -> Option<serde_json::Value> {
    // AP 実体を持つ投稿のリポストは、push 配送（deliver_ap_announce）と同じ Announce。
    if row.repost_of_post_id.is_some() {
        if let Some(orig_id) = &row.orig_ap_object_id {
            let own_id = row.ap_object_id.as_deref()?;
            let mut cc = vec![followers_uri.to_string()];
            if let Some(orig_actor_uri) = &row.orig_actor_uri {
                cc.push(orig_actor_uri.clone());
            }
            return Some(serde_json::json!({
                "type": "Announce",
                "id": own_id,
                "actor": actor_uri,
                "published": row.created_at.to_rfc3339(),
                "to": [AS_PUBLIC],
                "cc": cc,
                "object": orig_id
            }));
        }
    }

    // 通常投稿と、Bsky にしか実体が無い投稿のリポスト（代替本文の Note）。
    let note = local_note(state, row.id, actor_id).await?;
    let mut activity =
        seiran_common::ap::deliver::local_note_create_activity(&state.local_domain, row.id, &note);
    if let Some(obj) = activity.as_object_mut() {
        obj.remove("@context");
    }
    Some(activity)
}
