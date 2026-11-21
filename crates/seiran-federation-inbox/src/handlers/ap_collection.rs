//! outbox・featured 等の AP コレクション応答で共有する組み立て部品。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use seiran_common::ap::plain_to_html;
use sqlx::PgPool;
use std::collections::HashMap;

pub const AS_PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";

/// `application/activity+json` の 200 応答。
pub fn activity_json(body: serde_json::Value) -> Response {
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/activity+json",
        )],
        Json(body),
    )
        .into_response()
}

/// 投稿群の添付ファイルを AP の `Document` として post_id ごとにまとめて取得する
/// （表示順 = `position` 順）。取得失敗時は添付なしとして扱う。
pub async fn fetch_attachment_documents(
    db: &PgPool,
    post_ids: &[i64],
) -> HashMap<i64, Vec<serde_json::Value>> {
    let mut att_map: HashMap<i64, Vec<serde_json::Value>> = HashMap::new();
    if post_ids.is_empty() {
        return att_map;
    }
    let rows = seiran_common::repository::note_extras::local_attachments_for_posts(db, post_ids)
        .await
        .unwrap_or_default();
    for r in rows {
        let doc = seiran_common::ap::local_attachment_document(&r);
        att_map.entry(r.post_id).or_default().push(doc);
    }
    att_map
}

/// 公開コレクションに並べるローカルアクターの投稿者情報。
pub struct CollectionOwner<'a> {
    pub actor_uri: &'a str,
    pub followers_uri: &'a str,
}

/// 公開（to: Public, cc: followers）の Note オブジェクト。
pub fn public_note(
    owner: &CollectionOwner<'_>,
    note_id: &str,
    body: &str,
    published: &str,
    attachments: Vec<serde_json::Value>,
) -> serde_json::Value {
    let mut note_obj = serde_json::json!({
        "type": "Note",
        "id": note_id,
        "attributedTo": owner.actor_uri,
        "content": plain_to_html(body),
        "published": published,
        "to": [AS_PUBLIC],
        "cc": [owner.followers_uri],
        "url": note_id
    });
    if !attachments.is_empty() {
        seiran_common::ap::apply_note_sensitive(&mut note_obj, &attachments);
        note_obj["attachment"] = serde_json::Value::Array(attachments);
    }
    note_obj
}

/// Note を包む公開の Create アクティビティ。
pub fn public_create(
    owner: &CollectionOwner<'_>,
    activity_id: &str,
    published: &str,
    note_obj: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "type": "Create",
        "id": activity_id,
        "actor": owner.actor_uri,
        "published": published,
        "to": [AS_PUBLIC],
        "cc": [owner.followers_uri],
        "object": note_obj
    })
}
