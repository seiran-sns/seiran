use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::AppState;

#[derive(Deserialize)]
pub struct GetRecordParams {
    pub repo: String,
    pub collection: String,
    pub rkey: String,
}

#[derive(Serialize)]
pub struct GetRecordResponse {
    pub uri: String,
    pub cid: String,
    pub value: serde_json::Value,
}

pub async fn xrpc_get_record(
    Query(params): Query<GetRecordParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if params.collection != "app.bsky.feed.post" {
        return (StatusCode::NOT_FOUND, "collection が未対応です").into_response();
    }

    let row = sqlx::query(
        "SELECT p.body, p.created_at, p.at_uri, p.at_cid
         FROM posts p
         JOIN actors a ON a.id = p.actor_id
         WHERE a.at_did = $1 AND p.at_rkey = $2 AND p.deleted_at IS NULL
         LIMIT 1",
    )
    .bind(&params.repo)
    .bind(&params.rkey)
    .fetch_optional(&state.db)
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        _ => return (StatusCode::NOT_FOUND, "レコードが見つかりません").into_response(),
    };

    let body: String = match row.try_get("body") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[getRecord] body 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let created_at: chrono::DateTime<chrono::Utc> =
        row.try_get("created_at").unwrap_or_else(|_| chrono::Utc::now());
    let at_uri: String = match row.try_get("at_uri") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[getRecord] at_uri 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let at_cid: String = match row.try_get("at_cid") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[getRecord] at_cid 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let value = serde_json::json!({
        "$type": "app.bsky.feed.post",
        "text": body,
        "createdAt": created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });

    Json(GetRecordResponse { uri: at_uri, cid: at_cid, value }).into_response()
}
