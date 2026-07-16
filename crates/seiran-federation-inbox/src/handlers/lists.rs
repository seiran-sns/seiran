use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use sqlx::Row;
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
            tracing::error!("[Lists] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let rows = sqlx::query("SELECT id FROM lists WHERE owner_actor_id = $1 AND is_public = true ORDER BY created_at ASC")
        .bind(actor_id)
        .fetch_all(&state.db)
        .await;

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
        .filter_map(|r| r.try_get::<i64, _>("id").ok())
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
        [(axum::http::header::CONTENT_TYPE, "application/activity+json")],
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

    let row = sqlx::query(
        "SELECT l.id FROM lists l
         JOIN actors a ON a.id = l.owner_actor_id
         WHERE l.id = $1 AND a.username = $2 AND a.domain = $3
           AND a.actor_type = 'local' AND l.is_public = true",
    )
    .bind(list_id)
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::error!("[Lists] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    }

    let member_rows = sqlx::query(
        "SELECT a.actor_type::text AS actor_type, a.username, a.domain, a.ap_uri
         FROM list_members lm
         JOIN actors a ON a.id = lm.actor_id
         WHERE lm.list_id = $1 AND a.actor_type <> 'bsky'
         ORDER BY lm.added_at DESC",
    )
    .bind(list_id)
    .fetch_all(&state.db)
    .await;

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
        .filter_map(|r| {
            let actor_type: String = r.try_get("actor_type").ok()?;
            if actor_type == "local" {
                let username: String = r.try_get("username").ok()?;
                Some(format!("{}/users/{}", base, username))
            } else {
                r.try_get::<Option<String>, _>("ap_uri").ok().flatten()
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
        [(axum::http::header::CONTENT_TYPE, "application/activity+json")],
        Json(body),
    )
        .into_response()
}
