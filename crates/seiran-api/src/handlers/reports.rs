use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use seiran_common::generate_snowflake_id;
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, middleware::extract_auth, AppState};

/// Bluesky公式（`tools.ozone.report.defs`）準拠の通報理由。第1段階（カテゴリ）は
/// クライアント側の表示分類のみに使い、DBにはこの第2段階のトークン名をそのまま保存する。
const REASONS: [&str; 39] = [
    // 誤解を招くこと
    "reasonMisleadingBot",
    "reasonMisleadingImpersonation",
    "reasonMisleadingSpam",
    "reasonMisleadingScam",
    "reasonMisleadingElections",
    "reasonMisleadingOther",
    // 成人向けコンテンツ
    "reasonSexualAbuseContent",
    "reasonSexualNCII",
    "reasonSexualDeepfake",
    "reasonSexualAnimal",
    "reasonSexualUnlabeled",
    "reasonSexualOther",
    // 嫌がらせまたはヘイト
    "reasonHarassmentTroll",
    "reasonHarassmentTargeted",
    "reasonHarassmentHateSpeech",
    "reasonHarassmentDoxxing",
    "reasonHarassmentOther",
    // 暴力
    "reasonViolenceAnimal",
    "reasonViolenceThreats",
    "reasonViolenceGraphicContent",
    "reasonViolenceGlorification",
    "reasonViolenceExtremistContent",
    "reasonViolenceTrafficking",
    "reasonViolenceOther",
    // 児童の安全
    "reasonChildSafetyCSAM",
    "reasonChildSafetyGroom",
    "reasonChildSafetyPrivacy",
    "reasonChildSafetyHarassment",
    "reasonChildSafetyOther",
    // 自傷・危険行動
    "reasonSelfHarmContent",
    "reasonSelfHarmED",
    "reasonSelfHarmStunts",
    "reasonSelfHarmSubstances",
    "reasonSelfHarmOther",
    // サイトルール違反
    "reasonRuleSiteSecurity",
    "reasonRuleProhibitedSales",
    "reasonRuleBanEvasion",
    "reasonRuleOther",
    // その他
    "reasonOther",
];

#[derive(Debug, Deserialize)]
pub struct CreateReportRequest {
    pub subject_type: String,
    pub subject_actor_id: String,
    pub subject_post_id: Option<String>,
    pub reason_type: String,
    #[serde(default)]
    pub reason_text: String,
}

#[derive(Debug, Serialize)]
pub struct CreateReportResponse {
    pub id: String,
}

pub async fn create_report(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateReportRequest>,
) -> Result<(StatusCode, Json<CreateReportResponse>), ApiError> {
    let auth = extract_auth(&headers, &state.local_auth, state.app_tokens.as_ref()).await?;
    let reporter = state
        .actors
        .find_local_by_user_id(auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("REPORTER_NOT_FOUND"))?;
    let subject_actor_id = req
        .subject_actor_id
        .parse::<i64>()
        .map_err(|_| ApiError::BadRequest("INVALID_SUBJECT".into()))?;
    let subject_post_id = req
        .subject_post_id
        .as_deref()
        .map(str::parse::<i64>)
        .transpose()
        .map_err(|_| ApiError::BadRequest("INVALID_SUBJECT".into()))?;
    if !matches!(req.subject_type.as_str(), "actor" | "post")
        || (req.subject_type == "actor" && subject_post_id.is_some())
        || (req.subject_type == "post" && subject_post_id.is_none())
        || !REASONS.contains(&req.reason_type.as_str())
    {
        return Err(ApiError::BadRequest("INVALID_REPORT".into()));
    }
    if req.reason_text.chars().count() > 300 || req.reason_text.len() > 1000 {
        return Err(ApiError::BadRequest("REPORT_TEXT_TOO_LONG".into()));
    }
    let subject = state
        .actors
        .find_by_id(subject_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("SUBJECT_NOT_FOUND"))?;
    if subject.id == reporter.id {
        return Err(ApiError::BadRequest("CANNOT_REPORT_SELF".into()));
    }
    if let Some(post_id) = subject_post_id {
        let owner: Option<i64> =
            sqlx::query_scalar("SELECT actor_id FROM posts WHERE id=$1 AND deleted_at IS NULL")
                .bind(post_id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        if owner != Some(subject_actor_id) {
            return Err(ApiError::NotFound("SUBJECT_NOT_FOUND"));
        }
    }
    // 通報者は送信先を選ばない。すべての通報はまずローカル管理者に届き、
    // 対象がリモートの場合のみ、管理者が判断してFedi/Bskyへ転送する。
    let destination = if subject.actor_type != "local" {
        "remote"
    } else {
        "local"
    };
    let remote_host = (destination == "remote").then(|| subject.domain.clone());
    let id = generate_snowflake_id(chrono::Utc::now());
    sqlx::query(
        "INSERT INTO reports(id,reporter_actor_id,subject_type,subject_actor_id,subject_post_id,\
         reason_type,reason_text,destination,remote_host) \
         VALUES($1,$2,$3::report_subject_type,$4,$5,$6,$7,$8::report_destination,$9)",
    )
    .bind(id)
    .bind(reporter.id)
    .bind(&req.subject_type)
    .bind(subject_actor_id)
    .bind(subject_post_id)
    .bind(&req.reason_type)
    .bind(req.reason_text.trim())
    .bind(destination)
    .bind(remote_host)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((
        StatusCode::CREATED,
        Json(CreateReportResponse { id: id.to_string() }),
    ))
}
