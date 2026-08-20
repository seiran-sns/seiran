use axum::{extract::State, http::HeaderMap, response::IntoResponse, Json};
use chrono::{DateTime, NaiveDate};
use serde::Deserialize;

use super::{extract_bearer, service_did};
use crate::error::ApiError;
use crate::AppState;

const PERSONAL_DETAILS_PREF_TYPE: &str = "app.bsky.actor.defs#personalDetailsPref";

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

/// `YYYY-MM-DD`または完全なRFC3339タイムスタンプの両方を受け付ける
/// （Bluesky公式クライアントは`birthDate`をRFC3339で送ってくる）。
fn parse_birth_date(s: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.date_naive())
        .ok()
        .or_else(|| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

/// `preferences`配列から`#personalDetailsPref`要素を取り除いたものを返す。
fn strip_personal_details_pref(preferences: &serde_json::Value) -> Vec<serde_json::Value> {
    preferences
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|v| {
                    v.get("$type").and_then(|t| t.as_str()) != Some(PERSONAL_DETAILS_PREF_TYPE)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// `app.bsky.actor.getPreferences` — クライアント設定（フィード設定、ミュートワード等）を
/// 返す。中身は解釈せず不透明なJSON配列としてそのまま保存・返却するが、`#personalDetailsPref`
/// （年齢確認のbirthDate）だけは特別扱いし、`actors.birth_date`から都度生成して差し込む
/// （`actors.birth_date`が真実の源。seiranのUI/API・Fediverse連合とも共有する値のため）。
pub async fn xrpc_get_preferences(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = match authenticate_and_get_actor_id(&state, &headers).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let mut preferences = match state.atp_preferences.get(actor_id).await {
        Ok(p) => strip_personal_details_pref(&p),
        Err(e) => {
            return ApiError::Internal(format!("[getPreferences] DB エラー: {}", e)).into_response()
        }
    };
    match state.actors.find_birth_date(actor_id).await {
        Ok(Some(birth_date)) => {
            preferences.push(serde_json::json!({
                "$type": PERSONAL_DETAILS_PREF_TYPE,
                "birthDate": format!("{}T00:00:00.000Z", birth_date.format("%Y-%m-%d")),
            }));
        }
        Ok(None) => {}
        Err(e) => {
            return ApiError::Internal(format!("[getPreferences] 生年月日取得失敗: {}", e))
                .into_response()
        }
    }
    Json(serde_json::json!({ "preferences": preferences })).into_response()
}

#[derive(Deserialize)]
pub struct PutPreferencesRequest {
    pub preferences: serde_json::Value,
}

/// `app.bsky.actor.putPreferences` — `preferences`配列を丸ごと置き換える（全置換が仕様）。
/// `#personalDetailsPref`が含まれていれば`birthDate`を`actors.birth_date`へ反映し、
/// その要素自体は`atp_preferences`には保存しない（`actors.birth_date`と二重管理しない）。
pub async fn xrpc_put_preferences(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<PutPreferencesRequest>,
) -> impl IntoResponse {
    let actor_id = match authenticate_and_get_actor_id(&state, &headers).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    if let Some(arr) = req.preferences.as_array() {
        if let Some(pref) = arr
            .iter()
            .find(|v| v.get("$type").and_then(|t| t.as_str()) == Some(PERSONAL_DETAILS_PREF_TYPE))
        {
            let birth_date = pref
                .get("birthDate")
                .and_then(|v| v.as_str())
                .and_then(parse_birth_date);
            if let Err(e) = state
                .actors
                .update_birth_date_by_actor_id(actor_id, birth_date)
                .await
            {
                return ApiError::Internal(format!("[putPreferences] 生年月日更新失敗: {}", e))
                    .into_response();
            }
        }
    }

    let rest = strip_personal_details_pref(&req.preferences);
    match state
        .atp_preferences
        .put(actor_id, &serde_json::Value::Array(rest))
        .await
    {
        Ok(()) => Json(serde_json::json!({})).into_response(),
        Err(e) => ApiError::Internal(format!("[putPreferences] DB エラー: {}", e)).into_response(),
    }
}
