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
    // 投稿は専用パス
    if params.collection == "app.bsky.feed.post" {
        return get_record_post(&params, &state).await;
    }

    // それ以外は atp_records + atp_blocks から返す
    get_record_from_atp_records(&params, &state).await
}

async fn get_record_post(params: &GetRecordParams, state: &AppState) -> axum::response::Response {
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

/// `atp_records` テーブルから CID を引き、`atp_blocks` テーブルの CBOR を JSON にして返す。
async fn get_record_from_atp_records(
    params: &GetRecordParams,
    state: &AppState,
) -> axum::response::Response {
    // repo（DID または handle）からアクター取得
    let actor = match state.actors.find_by_did(&params.repo).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            // did: でなければ username として検索（ローカルアクター）
            match state.actors.find_by_username_domain(&params.repo, &state.local_domain).await {
                Ok(Some(a)) => a,
                _ => return (StatusCode::NOT_FOUND, "リポジトリが見つかりません").into_response(),
            }
        }
        Err(e) => {
            eprintln!("[getRecord] アクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // atp_records から CID を取得
    let record_row = sqlx::query(
        "SELECT cid FROM atp_records
         WHERE actor_id = $1 AND collection = $2 AND rkey = $3 LIMIT 1",
    )
    .bind(actor.id)
    .bind(&params.collection)
    .bind(&params.rkey)
    .fetch_optional(&state.db)
    .await;

    let cid_str = match record_row {
        Ok(Some(row)) => row.try_get::<String, _>("cid").unwrap_or_default(),
        Ok(None) => return (StatusCode::NOT_FOUND, "レコードが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[getRecord] atp_records 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // atp_blocks から CBOR バイト列を取得
    let block_row = sqlx::query(
        "SELECT bytes FROM atp_blocks WHERE cid = $1 AND actor_id = $2 LIMIT 1",
    )
    .bind(&cid_str)
    .bind(actor.id)
    .fetch_optional(&state.db)
    .await;

    let cbor_bytes: Vec<u8> = match block_row {
        Ok(Some(row)) => row.try_get::<Vec<u8>, _>("bytes").unwrap_or_default(),
        Ok(None) => return (StatusCode::NOT_FOUND, "ブロックが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[getRecord] atp_blocks 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // DAG-CBOR → serde_json::Value
    let value: serde_json::Value = match serde_ipld_dagcbor::from_slice(&cbor_bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[getRecord] CBOR デコード失敗 (cid={}): {}", cid_str, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "CBOR デコード失敗").into_response();
        }
    };

    let at_did = actor.at_did.as_deref().unwrap_or(&params.repo);
    let uri = format!("at://{}/{}/{}", at_did, params.collection, params.rkey);

    Json(GetRecordResponse { uri, cid: cid_str, value }).into_response()
}
