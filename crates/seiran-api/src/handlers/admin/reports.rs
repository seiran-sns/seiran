use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use seiran_common::atp::sign_service_auth_jwt;
use seiran_common::generate_snowflake_id;
use serde::{Deserialize, Serialize};

use crate::{
    error::ApiError,
    middleware::{require_admin, AuthUser},
    AppState,
};

#[derive(Debug, sqlx::FromRow)]
struct ReportRow {
    id: i64,
    reporter_actor_id: i64,
    reporter: String,
    subject_type: String,
    subject_actor_id: i64,
    subject: String,
    subject_post_id: Option<i64>,
    reason_type: String,
    reason_text: String,
    destination: String,
    remote_host: Option<String>,
    status: String,
    forwarded_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

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

async fn authorize(headers: &HeaderMap, state: &AppState) -> Result<AuthUser, ApiError> {
    require_admin(
        headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
}

pub async fn list_reports(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<ReportResponse>>, ApiError> {
    authorize(&headers, &state).await?;
    let rows = sqlx::query_as::<_, ReportRow>(
        "SELECT r.id,r.reporter_actor_id,concat(ra.username,'@',ra.domain) reporter,\
         r.subject_type::text subject_type,r.subject_actor_id,concat(sa.username,'@',sa.domain) subject,\
         r.subject_post_id,r.reason_type,r.reason_text,r.destination::text destination,r.remote_host,\
         r.status::text status,r.forwarded_at,r.closed_at,r.created_at FROM reports r \
         JOIN actors ra ON ra.id=r.reporter_actor_id JOIN actors sa ON sa.id=r.subject_actor_id \
         ORDER BY (r.status='open') DESC,r.created_at DESC"
    ).fetch_all(&state.db).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

pub async fn close_report(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    authorize(&headers, &state).await?;
    let done = sqlx::query("UPDATE reports SET status='closed',closed_at=NOW() WHERE id=$1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound("REPORT_NOT_FOUND"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_comments(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<CommentResponse>>, ApiError> {
    authorize(&headers, &state).await?;
    let rows = sqlx::query_as::<_, (i64,String,String,DateTime<Utc>)>(
        "SELECT c.id,c.body,u.email,c.created_at FROM report_comments c JOIN users u ON u.id=c.author_user_id \
         WHERE c.report_id=$1 ORDER BY c.created_at"
    ).bind(id).fetch_all(&state.db).await.map_err(|e| ApiError::Internal(e.to_string()))?;
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
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<CommentRequest>,
) -> Result<(StatusCode, Json<CommentResponse>), ApiError> {
    let admin = authorize(&headers, &state).await?;
    let body = req.body.trim();
    if body.is_empty() || body.chars().count() > 2000 {
        return Err(ApiError::BadRequest("INVALID_REPORT_COMMENT".into()));
    }
    let comment_id = generate_snowflake_id(Utc::now());
    let created_at:DateTime<Utc>=sqlx::query_scalar(
        "INSERT INTO report_comments(id,report_id,author_user_id,body) VALUES($1,$2,$3,$4) RETURNING created_at"
    ).bind(comment_id).bind(id).bind(admin.user_id).bind(body).fetch_one(&state.db).await
     .map_err(|e|ApiError::Internal(e.to_string()))?;
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
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    authorize(&headers, &state).await?;
    let done=sqlx::query(
        "UPDATE posts SET deleted_at=NOW() WHERE id=(SELECT subject_post_id FROM reports WHERE id=$1) AND deleted_at IS NULL"
    ).bind(id).execute(&state.db).await.map_err(|e|ApiError::Internal(e.to_string()))?;
    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound("SUBJECT_NOT_FOUND"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn suspend_subject(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    authorize(&headers, &state).await?;
    let user_id: Option<i64> = sqlx::query_scalar(
        "SELECT a.user_id FROM reports r JOIN actors a ON a.id=r.subject_actor_id WHERE r.id=$1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .flatten();
    let Some(user_id) = user_id else {
        return Err(ApiError::BadRequest(
            "REMOTE_USER_CANNOT_BE_SUSPENDED".into(),
        ));
    };
    state
        .users
        .set_suspended(user_id, true)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(sqlx::FromRow)]
struct ForwardRow {
    reporter_ap_uri: Option<String>,
    reporter_ap_key: Option<String>,
    reporter_did: Option<String>,
    subject_ap_uri: Option<String>,
    subject_inbox: Option<String>,
    subject_did: Option<String>,
    subject_post_ap_uri: Option<String>,
    subject_post_at_uri: Option<String>,
    subject_post_at_cid: Option<String>,
    reason_type: String,
    reason_text: String,
    destination: String,
}

pub async fn forward_report(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    authorize(&headers, &state).await?;
    let row=sqlx::query_as::<_,ForwardRow>(
        "SELECT ra.ap_uri reporter_ap_uri,ra.at_signing_key_pem reporter_ap_key,ra.at_did reporter_did,\
         sa.ap_uri subject_ap_uri,sa.ap_inbox_url subject_inbox,sa.at_did subject_did,\
         p.ap_object_id subject_post_ap_uri,p.at_uri subject_post_at_uri,p.at_cid subject_post_at_cid,\
         r.reason_type,r.reason_text,r.destination::text destination FROM reports r \
         JOIN actors ra ON ra.id=r.reporter_actor_id JOIN actors sa ON sa.id=r.subject_actor_id \
         LEFT JOIN posts p ON p.id=r.subject_post_id WHERE r.id=$1"
    ).bind(id).fetch_optional(&state.db).await.map_err(|e|ApiError::Internal(e.to_string()))?
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
            content = format!("{}\n\n対象投稿: {}", content, post_uri);
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
    sqlx::query("UPDATE reports SET forwarded_at=NOW() WHERE id=$1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
