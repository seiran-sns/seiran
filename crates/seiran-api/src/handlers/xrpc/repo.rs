use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

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

    let record = match state.posts.find_record(&params.repo, &params.rkey).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "レコードが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[getRecord] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let value = serde_json::json!({
        "$type": "app.bsky.feed.post",
        "text": record.body,
        "createdAt": record.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });

    Json(GetRecordResponse { uri: record.at_uri, cid: record.at_cid, value }).into_response()
}
