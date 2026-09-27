use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;

use super::ap_collection::{
    activity_json, fetch_attachment_documents, public_note, CollectionOwner,
};
use crate::AppState;

/// GET /users/:username/collections/featured
/// ピン留め投稿（#61）を `OrderedCollection` として返す。Mastodon 等の実装はプロフィール
/// 取得時にこのコレクションを都度フェッチしてピン留め表示を更新する（Add/Remove Activity
/// の配送は行わない、最大5件のためページングも行わない）。
pub async fn featured_handler(
    Path(username): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let actor_row =
        seiran_common::repository::ap_public::live_local_actor_id(&state.db, &username).await;

    let actor_id: i64 = match actor_row {
        Ok(Some(id)) => id,
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

    // このエンドポイントは認証なしの完全匿名アクセスのため、followers_only/direct な
    // ピン留め投稿は常に除外する（可視性による閲覧制御）。
    let rows = seiran_common::repository::ap_public::featured_posts(&state.db, actor_id).await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[Featured] 投稿取得エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let post_ids: Vec<i64> = rows.iter().map(|(id, _, _)| *id).collect();
    let mut att_map = fetch_attachment_documents(&state.db, &post_ids).await;
    let owner = CollectionOwner {
        actor_uri: &actor_uri,
        followers_uri: &followers_uri,
    };

    // featured collection は Note オブジェクトを直接（Create でラップせずに）並べる
    // （Mastodon 等の実装と同じ慣習）。
    let mut ordered_items = Vec::new();
    for (post_id, body, created_at) in rows {
        let note_id = format!("{}/notes/{}", base, post_id);
        let attachments = att_map.remove(&post_id).unwrap_or_default();
        ordered_items.push(public_note(
            &owner,
            &note_id,
            &body,
            &created_at.to_rfc3339(),
            attachments,
        ));
    }

    let body = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "OrderedCollection",
        "id": featured_uri,
        "totalItems": ordered_items.len(),
        "orderedItems": ordered_items
    });

    activity_json(body)
}
