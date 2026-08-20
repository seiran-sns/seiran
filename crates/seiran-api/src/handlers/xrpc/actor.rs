use axum::{extract::State, http::HeaderMap, response::IntoResponse, Json};
use serde::Deserialize;

use super::{extract_bearer, service_did};
use crate::error::ApiError;
use crate::AppState;

/// accessJwtを検証し、対応する`actor_id`を返す。`getPreferences`/`putPreferences`は
/// 本人のみアクセス可能（他人の設定を覗き見/上書きできてはならない）。
async fn authenticate_and_get_actor_id(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<i64, axum::response::Response> {
    let Some(token) = extract_bearer(headers) else {
        return Err(ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response());
    };
    let verified = state
        .local_auth
        .verify_atp_access_token(token, &service_did(state))
        .map_err(|_| ApiError::Unauthorized("トークンが無効です").into_response())?;
    match state.actors.find_by_did(&verified.did).await {
        Ok(Some(actor)) => Ok(actor.id),
        Ok(None) => Err(ApiError::Unauthorized("アクターが見つかりません").into_response()),
        Err(e) => Err(
            ApiError::Internal(format!("[getPreferences] アクター解決失敗: {}", e)).into_response(),
        ),
    }
}

/// `app.bsky.actor.getPreferences` — クライアント設定（年齢確認のbirthDate、フィード設定、
/// ミュートワード等）を返す。中身は解釈せず不透明なJSON配列としてそのまま保存・返却する。
pub async fn xrpc_get_preferences(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = match authenticate_and_get_actor_id(&state, &headers).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.atp_preferences.get(actor_id).await {
        Ok(preferences) => Json(serde_json::json!({ "preferences": preferences })).into_response(),
        Err(e) => ApiError::Internal(format!("[getPreferences] DB エラー: {}", e)).into_response(),
    }
}

#[derive(Deserialize)]
pub struct PutPreferencesRequest {
    pub preferences: serde_json::Value,
}

/// `app.bsky.actor.putPreferences` — `preferences`配列を丸ごと置き換える（全置換が仕様）。
pub async fn xrpc_put_preferences(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<PutPreferencesRequest>,
) -> impl IntoResponse {
    let actor_id = match authenticate_and_get_actor_id(&state, &headers).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.atp_preferences.put(actor_id, &req.preferences).await {
        Ok(()) => Json(serde_json::json!({})).into_response(),
        Err(e) => ApiError::Internal(format!("[putPreferences] DB エラー: {}", e)).into_response(),
    }
}
