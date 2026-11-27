use axum::{extract::State, response::IntoResponse, Json};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::Deserialize;

use super::auth::{authenticate, ScopeNeed, XrpcRequest};
use crate::error::ApiError;
use crate::AppState;

const PERSONAL_DETAILS_PREF_TYPE: &str = "app.bsky.actor.defs#personalDetailsPref";
/// 生年月日から都度計算して返す読み取り専用の設定。クライアントが送ってきても保存しない
/// （保存すると誕生日を過ぎても古い年齢区分が返り続ける）。
const DECLARED_AGE_PREF_TYPE: &str = "app.bsky.actor.defs#declaredAgePref";

/// 設定を読み書きする本人。
struct PrefCaller {
    actor_id: i64,
    /// メインパスワードのセッションだけ真。生年月日（`#personalDetailsPref`）を読み書きできるのは
    /// これだけで、アプリパスワード・OAuth のクライアントには年齢区分（`#declaredAgePref`）しか
    /// 見せない（生年月日そのものは個人情報なので、年齢制限の判定に要る範囲だけ渡す）。
    privileged: bool,
}

/// accessJwtを検証し、本人を返す。`getPreferences`/`putPreferences`は
/// 本人のみアクセス可能（他人の設定を覗き見/上書きできてはならない）。
async fn authenticate_pref_caller(
    state: &AppState,
    xrpc: &XrpcRequest,
) -> Result<PrefCaller, axum::response::Response> {
    let verified = authenticate(state, xrpc, ScopeNeed::Generic).await?;
    match state.actors.find_by_did(&verified.did).await {
        Ok(Some(actor)) => Ok(PrefCaller {
            actor_id: actor.id,
            privileged: verified.privileged,
        }),
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

fn pref_type(v: &serde_json::Value) -> Option<&str> {
    v.get("$type").and_then(|t| t.as_str())
}

/// `preferences`配列から、保存しない要素（`#personalDetailsPref`・`#declaredAgePref`）を
/// 取り除いたものを返す。どちらも`actors.birth_date`から都度生成する。
fn strip_birth_date_prefs(preferences: &serde_json::Value) -> Vec<serde_json::Value> {
    preferences
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|v| {
                    !matches!(
                        pref_type(v),
                        Some(PERSONAL_DETAILS_PREF_TYPE | DECLARED_AGE_PREF_TYPE)
                    )
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// `today`時点の満年齢。
fn age_on(birth_date: NaiveDate, today: NaiveDate) -> i32 {
    let mut age = today.year() - birth_date.year();
    if (today.month(), today.day()) < (birth_date.month(), birth_date.day()) {
        age -= 1;
    }
    age
}

/// 生年月日から返す設定要素。メインパスワードのセッションには生年月日と年齢区分の両方、
/// それ以外には年齢区分だけを返す（Bluesky 公式クライアントは生年月日が読めなければ
/// 年齢区分で年齢制限を判定する）。
fn birth_date_prefs(
    birth_date: NaiveDate,
    privileged: bool,
    today: NaiveDate,
) -> Vec<serde_json::Value> {
    let age = age_on(birth_date, today);
    let mut prefs = Vec::new();
    if privileged {
        prefs.push(serde_json::json!({
            "$type": PERSONAL_DETAILS_PREF_TYPE,
            "birthDate": format!("{}T00:00:00.000Z", birth_date.format("%Y-%m-%d")),
        }));
    }
    prefs.push(serde_json::json!({
        "$type": DECLARED_AGE_PREF_TYPE,
        "isOverAge13": age >= 13,
        "isOverAge16": age >= 16,
        "isOverAge18": age >= 18,
    }));
    prefs
}

/// `app.bsky.actor.getPreferences` — クライアント設定（フィード設定、ミュートワード等）を
/// 返す。中身は解釈せず不透明なJSON配列としてそのまま保存・返却するが、`#personalDetailsPref`
/// （年齢確認のbirthDate）と`#declaredAgePref`（年齢区分）だけは特別扱いし、
/// `actors.birth_date`から都度生成して差し込む（`actors.birth_date`が真実の源。
/// seiranのUI/API・Fediverse連合とも共有する値のため）。
pub async fn xrpc_get_preferences(
    xrpc: XrpcRequest,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let caller = match authenticate_pref_caller(&state, &xrpc).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let mut preferences = match state.atp_preferences.get(caller.actor_id).await {
        Ok(p) => strip_birth_date_prefs(&p),
        Err(e) => {
            return ApiError::Internal(format!("[getPreferences] DB エラー: {}", e)).into_response()
        }
    };
    match state.actors.find_birth_date(caller.actor_id).await {
        Ok(Some(birth_date)) => {
            preferences.extend(birth_date_prefs(
                birth_date,
                caller.privileged,
                Utc::now().date_naive(),
            ));
        }
        Ok(None) => {}
        Err(e) => {
            return ApiError::Internal(format!("[getPreferences] 生年月日取得失敗: {}", e))
                .into_response()
        }
    }
    Json(serde_json::json!({ "preferences": preferences })).into_response()
}

/// `app.bsky.unspecced.getTrends` — トレンド機能は未実装のため常に空配列を返す。
/// 実装が無いと`atproto-proxy`ヘッダー無しで呼ぶbsky.appクライアントに対し
/// 404（`xrpc_proxy_fallback`のMethodNotImplemented）を返してしまい、表示側で
/// エラー扱いになる。空のトレンド一覧は仕様上妥当な応答。
pub async fn xrpc_get_trends() -> impl IntoResponse {
    Json(serde_json::json!({ "trends": [] }))
}

#[derive(Deserialize)]
pub struct PutPreferencesRequest {
    pub preferences: serde_json::Value,
}

/// `app.bsky.actor.putPreferences` — `preferences`配列を丸ごと置き換える（全置換が仕様）。
/// `#personalDetailsPref`が含まれていれば`birthDate`を`actors.birth_date`へ反映し、
/// その要素自体は`atp_preferences`には保存しない（`actors.birth_date`と二重管理しない）。
/// メインパスワード以外のセッションが`#personalDetailsPref`を送ってきたら拒否する
/// （読めない値を書き換えさせない）。`#declaredAgePref`は読み取り専用なので黙って捨てる。
pub async fn xrpc_put_preferences(
    xrpc: XrpcRequest,
    State(state): State<AppState>,
    Json(req): Json<PutPreferencesRequest>,
) -> impl IntoResponse {
    let caller = match authenticate_pref_caller(&state, &xrpc).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let actor_id = caller.actor_id;

    if let Some(arr) = req.preferences.as_array() {
        if let Some(pref) = arr
            .iter()
            .find(|v| pref_type(v) == Some(PERSONAL_DETAILS_PREF_TYPE))
        {
            if !caller.privileged {
                return ApiError::BadRequest(format!(
                    "この認証では次の設定を変更できません: {}",
                    PERSONAL_DETAILS_PREF_TYPE
                ))
                .into_response();
            }
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

    let rest = strip_birth_date_prefs(&req.preferences);
    match state
        .atp_preferences
        .put(actor_id, &serde_json::Value::Array(rest))
        .await
    {
        Ok(()) => Json(serde_json::json!({})).into_response(),
        Err(e) => ApiError::Internal(format!("[putPreferences] DB エラー: {}", e)).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn age_counts_up_on_birthday() {
        let birth = date("2008-11-27");
        assert_eq!(age_on(birth, date("2026-11-26")), 17);
        assert_eq!(age_on(birth, date("2026-11-27")), 18);
    }

    #[test]
    fn unprivileged_caller_gets_only_declared_age() {
        let prefs = birth_date_prefs(date("2010-01-01"), false, date("2026-11-27"));
        assert_eq!(
            prefs,
            vec![serde_json::json!({
                "$type": DECLARED_AGE_PREF_TYPE,
                "isOverAge13": true,
                "isOverAge16": true,
                "isOverAge18": false,
            })]
        );
    }

    #[test]
    fn privileged_caller_gets_birth_date_and_declared_age() {
        let prefs = birth_date_prefs(date("2000-05-01"), true, date("2026-11-27"));
        let types: Vec<_> = prefs.iter().filter_map(pref_type).collect();
        assert_eq!(
            types,
            vec![PERSONAL_DETAILS_PREF_TYPE, DECLARED_AGE_PREF_TYPE]
        );
        assert_eq!(prefs[0]["birthDate"], "2000-05-01T00:00:00.000Z");
    }

    #[test]
    fn stored_prefs_drop_birth_date_derived_entries() {
        let input = serde_json::json!([
            {"$type": PERSONAL_DETAILS_PREF_TYPE, "birthDate": "2000-01-01"},
            {"$type": DECLARED_AGE_PREF_TYPE, "isOverAge18": true},
            {"$type": "app.bsky.actor.defs#adultContentPref", "enabled": false},
        ]);
        let rest = strip_birth_date_prefs(&input);
        assert_eq!(rest.len(), 1);
        assert_eq!(
            pref_type(&rest[0]),
            Some("app.bsky.actor.defs#adultContentPref")
        );
    }
}
