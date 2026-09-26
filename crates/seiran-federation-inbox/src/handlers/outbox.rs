use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use seiran_common::ap::deliver::at_uri_to_bsky_app_url;
use serde::Deserialize;
use sqlx::PgPool;
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

/// outbox は認証なしの完全匿名アクセスのため、followers_only/direct な投稿は常に除外する
/// （featured と同じ可視性条件）。
const PUBLIC_POST_CONDITION: &str =
    "p.deleted_at IS NULL AND p.visibility NOT IN ('followers_only', 'direct')";

pub async fn outbox_handler(
    Path(username): Path<String>,
    Query(query): Query<OutboxQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (actor_id, total_items) = match find_outbox_owner(&state.db, &username).await {
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
    let rows = match fetch_outbox_rows(&state.db, actor_id, max_id).await {
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

/// outbox の持ち主（ローカルの未退会アクター）の id と、公開投稿の総数。
async fn find_outbox_owner(db: &PgPool, username: &str) -> Result<Option<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT a.id, COUNT(p.id) AS total
         FROM actors a
         LEFT JOIN posts p ON p.actor_id = a.id AND {PUBLIC_POST_CONDITION}
         WHERE a.username = $1 AND a.actor_type = 'local' AND a.withdrawn_at IS NULL
         GROUP BY a.id
         LIMIT 1"
    ))
    .bind(username)
    .fetch_optional(db)
    .await
}

/// outbox に並べる投稿1件。リポスト行は本文（body）が常に空文字列で、単独では Create(Note) として
/// 表現できない（元投稿を参照する Announce として表現する必要がある）ため、リポスト元（orig）の
/// ap_object_id / at_uri / 投稿者情報も合わせて持つ。
#[derive(sqlx::FromRow)]
struct OutboxRow {
    id: i64,
    body: String,
    created_at: chrono::DateTime<chrono::Utc>,
    repost_of_post_id: Option<i64>,
    ap_object_id: Option<String>,
    orig_ap_object_id: Option<String>,
    orig_at_uri: Option<String>,
    orig_username: Option<String>,
    orig_display_name: Option<String>,
    orig_actor_uri: Option<String>,
}

/// `max_id` より古い公開投稿を新しい順に最大 `PAGE_SIZE` 件取得する。
async fn fetch_outbox_rows(
    db: &PgPool,
    actor_id: i64,
    max_id: Option<i64>,
) -> Result<Vec<OutboxRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT p.id, p.body, p.created_at, p.repost_of_post_id, p.ap_object_id,
                orig.ap_object_id AS orig_ap_object_id, orig.at_uri AS orig_at_uri,
                oa.username AS orig_username, oa.display_name AS orig_display_name,
                oa.ap_uri AS orig_actor_uri
         FROM posts p
         LEFT JOIN posts orig ON orig.id = p.repost_of_post_id
         LEFT JOIN actors oa ON oa.id = orig.actor_id
         WHERE p.actor_id = $1 AND {PUBLIC_POST_CONDITION}
           AND ($2::BIGINT IS NULL OR p.id < $2)
         ORDER BY p.id DESC LIMIT $3"
    ))
    .bind(actor_id)
    .bind(max_id)
    .bind(PAGE_SIZE)
    .fetch_all(db)
    .await
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
