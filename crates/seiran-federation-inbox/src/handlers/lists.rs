use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::AppState;

/// GET /users/:username/lists
/// そのユーザーの公開リスト一覧を `OrderedCollection` で返す（Mastodon にはない独自拡張）。
/// `orderedItems` には個別リストのCollection URLを列挙する（`featured` と異なり
/// アイテムをインライン展開しない。リストは投稿と違い件数が読めないため）。
pub async fn lists_collection_handler(
    Path(username): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let actor_row =
        seiran_common::repository::ap_public::live_local_actor_id(&state.db, &username).await;

    let actor_id: i64 = match actor_row {
        Ok(Some(id)) => id,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::error!("[Lists] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let rows = seiran_common::repository::ap_public::public_list_ids(&state.db, actor_id).await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[Lists] 取得エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let base = format!("https://{}", state.local_domain);
    let lists_uri = format!("{}/users/{}/lists", base, username);
    let ordered_items: Vec<String> = rows
        .iter()
        .map(|id| format!("{}/{}", lists_uri, id))
        .collect();

    let body = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "OrderedCollection",
        "id": lists_uri,
        "totalItems": ordered_items.len(),
        "orderedItems": ordered_items
    });

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

/// GET /users/:username/lists/:list_id
/// 個別リストのメンバーを `OrderedCollection` で返す。要件により、Bskyメンバーは
/// 含めない（`actor_type <> 'bsky'` でフィルタ）。非公開リスト・他人のusernameとの
/// 不一致・存在しないリストはいずれも 404 にする（非公開リストの存在を漏らさない）。
pub async fn list_detail_handler(
    Path((username, list_id)): Path<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let Ok(list_id) = list_id.parse::<i64>() else {
        return (StatusCode::NOT_FOUND, "").into_response();
    };

    let row =
        seiran_common::repository::ap_public::is_public_list_of(&state.db, list_id, &username)
            .await;

    match row {
        Ok(true) => {}
        Ok(false) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::error!("[Lists] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    }

    let member_rows =
        seiran_common::repository::ap_public::public_list_members(&state.db, list_id).await;

    let member_rows = match member_rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[Lists] メンバー取得エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let base = format!("https://{}", state.local_domain);
    let ordered_items: Vec<String> = member_rows
        .iter()
        .filter_map(|(actor_type, username, ap_uri)| {
            if actor_type == "local" {
                Some(format!("{}/users/{}", base, username))
            } else {
                ap_uri.clone()
            }
        })
        .collect();

    let list_uri = format!("{}/users/{}/lists/{}", base, username, list_id);
    let body = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "OrderedCollection",
        "id": list_uri,
        "totalItems": ordered_items.len(),
        "orderedItems": ordered_items
    });

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
