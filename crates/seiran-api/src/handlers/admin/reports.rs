use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use seiran_common::atp::sign_service_auth_jwt;
use seiran_common::generate_snowflake_id;
use seiran_common::repository::report::{self, ReportRow};
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, middleware::AuthUser, AppState};

#[derive(Debug, Serialize)]
pub struct ReportResponse {
    pub id: String,
    pub reporter_actor_id: String,
    pub reporter: String,
    pub subject_type: String,
    pub subject_actor_id: String,
    pub subject: String,
    pub subject_post_id: Option<String>,
    pub reason_type: String,
    pub reason_text: String,
    pub destination: String,
    pub remote_host: Option<String>,
    pub status: String,
    pub forwarded_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub subject_suspended: bool,
}

impl From<ReportRow> for ReportResponse {
    fn from(r: ReportRow) -> Self {
        Self {
            id: r.id.to_string(),
            reporter_actor_id: r.reporter_actor_id.to_string(),
            reporter: r.reporter,
            subject_type: r.subject_type,
            subject_actor_id: r.subject_actor_id.to_string(),
            subject: r.subject,
            subject_post_id: r.subject_post_id.map(|v| v.to_string()),
            reason_type: r.reason_type,
            reason_text: r.reason_text,
            destination: r.destination,
            remote_host: r.remote_host,
            status: r.status,
            forwarded_at: r.forwarded_at,
            closed_at: r.closed_at,
            created_at: r.created_at,
            subject_suspended: r.subject_suspended,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CommentResponse {
    pub id: String,
    pub body: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Deserialize)]
pub struct CommentRequest {
    pub body: String,
}

// 通報の閲覧・対応は admin / moderator が行える（#179: moderator は調停者として
// 通報対応に必要な凍結・投稿削除・連合転送を含めて利用可能）。認可自体は
// ルータ層の`middleware::report_moderator_only`で強制済み（#221）。

pub async fn list_reports(
    State(state): State<AppState>,
) -> Result<Json<Vec<ReportResponse>>, ApiError> {
    let rows = report::list(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

pub async fn close_report(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let done = report::close(&state.db, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !done {
        return Err(ApiError::NotFound("REPORT_NOT_FOUND"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_comments(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<CommentResponse>>, ApiError> {
    let rows = report::list_comments(&state.db, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, body, author, created_at)| CommentResponse {
                id: id.to_string(),
                body,
                author,
                created_at,
            })
            .collect(),
    ))
}

pub async fn add_comment(
    Extension(admin): Extension<AuthUser>,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<CommentRequest>,
) -> Result<(StatusCode, Json<CommentResponse>), ApiError> {
    let body = req.body.trim();
    if body.is_empty() || body.chars().count() > 2000 {
        return Err(ApiError::BadRequest("INVALID_REPORT_COMMENT".into()));
    }
    let comment_id = generate_snowflake_id(Utc::now());
    let created_at = report::add_comment(&state.db, comment_id, id, admin.user_id, body)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok((
        StatusCode::CREATED,
        Json(CommentResponse {
            id: comment_id.to_string(),
            body: body.to_owned(),
            author: admin.email,
            created_at,
        }),
    ))
}

pub async fn delete_subject_post(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let done = report::delete_subject_post(&state.db, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !done {
        return Err(ApiError::NotFound("SUBJECT_NOT_FOUND"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn suspend_subject(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    // ローカル・リモートを問わず、通報対象アクターをそのまま凍結する（#凍結リモート対応で
    // REMOTE_USER_CANNOT_BE_SUSPENDED を撤去、actors.suspended_at がローカル・リモート共通の
    // enforcement を担うため actors.user_id の解決は不要になった）。
    let subject_actor_id = report::subject_actor_id(&state.db, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let subject_actor_id = subject_actor_id.ok_or(ApiError::NotFound("REPORT_NOT_FOUND"))?;
    state
        .actors
        .set_suspended(subject_actor_id, true)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn forward_report(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let row = report::forward_material(&state.db, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("REPORT_NOT_FOUND"))?;
    if row.destination != "remote" {
        return Err(ApiError::BadRequest("REPORT_IS_LOCAL".into()));
    }
    if let (Some(actor_uri), Some(private_key), Some(subject_uri), Some(inbox)) = (
        row.reporter_ap_uri,
        state.secrets.ap_private_key_pem.clone(),
        row.subject_ap_uri,
        row.subject_inbox,
    ) {
        // ActivityPubのFlagはアカウント通報のみを表現できるため、objectは常に対象Actorの
        // URIとする。投稿通報の場合は説明文に対象投稿のURLを付記して伝える。
        let mut content = if row.reason_text.is_empty() {
            row.reason_type
        } else {
            format!("[{}] {}", row.reason_type, row.reason_text)
        };
        if let Some(post_uri) = &row.subject_post_ap_uri {
            // Flagは相手サーバーの運営言語に関わらず読まれるため、定型文言は英語で統一する。
            content = format!("{}\n\nReported post: {}", content, post_uri);
        }
        let activity = serde_json::json!({
            "@context":"https://www.w3.org/ns/activitystreams",
            "id":format!("https://{}/reports/{}",state.local_domain,id),
            "type":"Flag","actor":actor_uri,"object":[subject_uri],"content":content
        });
        state
            .ap_client
            .sign_and_post(
                &inbox,
                &activity.to_string(),
                &format!("{}#main-key", actor_uri),
                &private_key,
            )
            .await
            .map_err(|e| ApiError::BadGateway(e.to_string()))?;
    } else if let (Some(reporter_did), Some(private_key), Some(subject_did)) =
        (row.reporter_did, row.reporter_ap_key, row.subject_did)
    {
        const MOD_DID: &str = "did:plc:ar7c4by46qjdydhdevvrndac";
        let jwt = sign_service_auth_jwt(
            &private_key,
            &reporter_did,
            MOD_DID,
            "com.atproto.moderation.createReport",
        )
        .map_err(|e| ApiError::Internal(e.to_string()))?;
        let subject = match (row.subject_post_at_uri, row.subject_post_at_cid) {
            (Some(uri), Some(cid)) => {
                serde_json::json!({"$type":"com.atproto.repo.strongRef","uri":uri,"cid":cid})
            }
            _ => serde_json::json!({"$type":"com.atproto.admin.defs#repoRef","did":subject_did}),
        };
        // reason_typeはtools.ozone.report.defsのトークン名（例: reasonMisleadingSpam）を
        // そのまま保持しているため、名前空間を付けるだけで送信できる。
        let response = state
            .http_client
            .post("https://mod.bsky.app/xrpc/com.atproto.moderation.createReport")
            .bearer_auth(jwt)
            .json(&serde_json::json!({
                "reasonType":format!("tools.ozone.report.defs#{}",row.reason_type),
                "reason":row.reason_text,"subject":subject,"modTool":{"name":"seiran"}
            }))
            .send()
            .await
            .map_err(|e| ApiError::BadGateway(e.to_string()))?;
        if !response.status().is_success() {
            return Err(ApiError::BadGateway(format!(
                "Bluesky moderation: {}",
                response.status()
            )));
        }
    } else {
        return Err(ApiError::BadRequest("REMOTE_REPORT_UNAVAILABLE".into()));
    }
    report::mark_forwarded(&state.db, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
