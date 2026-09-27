use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use seiran_common::ap::deliver::at_uri_to_bsky_app_url;
use seiran_common::repository::ap_public::OutboxRow;
use serde::Deserialize;
use std::sync::Arc;

use super::ap_collection::{
    activity_json, fetch_attachment_documents, public_create, public_note, CollectionOwner,
    AS_PUBLIC,
};
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

    let post_ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut att_map = fetch_attachment_documents(&state.db, &post_ids).await;

    let actor_uri = format!("{}/users/{}", base, username);
    let followers_uri = format!("{}/followers", actor_uri);
    let owner = CollectionOwner {
        actor_uri: &actor_uri,
        followers_uri: &followers_uri,
    };
    let ordered_items: Vec<serde_json::Value> = rows
        .iter()
        .filter_map(|row| {
            let attachments = att_map.remove(&row.id).unwrap_or_default();
            outbox_item(&base, &owner, row, attachments)
        })
        .collect();

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
fn outbox_item(
    base: &str,
    owner: &CollectionOwner<'_>,
    row: &OutboxRow,
    attachments: Vec<serde_json::Value>,
) -> Option<serde_json::Value> {
    let published = row.created_at.to_rfc3339();
    let activity_id = format!("{}/activities/{}", base, row.id);

    if row.repost_of_post_id.is_some() {
        return repost_item(owner, row, &activity_id, &published);
    }

    let note_id = row
        .ap_object_id
        .clone()
        .unwrap_or_else(|| format!("{}/notes/{}", base, row.id));
    let note_obj = public_note(owner, &note_id, &row.body, &published, attachments);
    Some(public_create(owner, &activity_id, &published, note_obj))
}

/// リポスト行: body は常に空文字列のため、push 配送（deliver_ap_announce /
/// deliver_post_to_ap_followers）と同じ表現で Announce または Create(Note)
/// として組み立てる。素通しで body を Create(Note) 化すると、push 側とは
/// 別の AP object id を持つ「空の通常ポスト」がリモートに二重出現する。
fn repost_item(
    owner: &CollectionOwner<'_>,
    row: &OutboxRow,
    activity_id: &str,
    published: &str,
) -> Option<serde_json::Value> {
    let own_id = row.ap_object_id.as_deref()?;

    if let Some(orig_id) = &row.orig_ap_object_id {
        let mut cc = vec![owner.followers_uri.to_string()];
        if let Some(orig_actor_uri) = &row.orig_actor_uri {
            cc.push(orig_actor_uri.clone());
        }
        return Some(serde_json::json!({
            "type": "Announce",
            "id": own_id,
            "actor": owner.actor_uri,
            "published": published,
            "to": [AS_PUBLIC],
            "cc": cc,
            "object": orig_id
        }));
    }

    // Bsky ネイティブ投稿のリポスト → Fedi フォールバック（テキスト投稿）。
    // push 配送側（deliver_repost）と同じ本文を組み立てる。
    // リポスト元がどちらの ID も持たない（削除済み等）場合は表現不能のためスキップ。
    let orig_at_uri = row.orig_at_uri.as_deref()?;
    let orig_username = row.orig_username.as_deref().unwrap_or_default();
    let author_name = row.orig_display_name.as_deref().unwrap_or(orig_username);
    let text = format!(
        "🔁 {}: {}",
        author_name,
        at_uri_to_bsky_app_url(orig_at_uri)
    );
    let note_obj = public_note(owner, own_id, &text, published, Vec::new());
    Some(public_create(owner, activity_id, published, note_obj))
}
