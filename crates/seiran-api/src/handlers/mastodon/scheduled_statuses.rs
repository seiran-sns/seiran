//! 予約投稿（`ScheduledStatus`）。予約の作成は `POST /api/v1/statuses` の `scheduled_at`。
//! 検証・保存・実行は `handlers::scheduled_posts` と共有し、ここは Mastodon の形への整形だけを持つ。
//! 投稿できなかった予約（seiran 独自）も一覧に含める（予約日時は過去になっている）。

use axum::{
    extract::{OriginalUri, Path, State},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::handlers::notes::dto::CreateNoteRequest;
use crate::handlers::scheduled_posts::{self as scheduled, ScheduledPostRow};
use crate::middleware::AuthedUser;
use crate::AppState;

use super::convert::{mastodon_timestamp, to_mastodon_visibility};
use super::extract::{MastodonParams, MastodonQuery, PageParams};
use super::statuses::parse_id;
use super::types::MastodonMediaAttachment;

#[derive(Serialize)]
pub struct MastodonScheduledStatus {
    pub id: String,
    pub scheduled_at: String,
    pub params: MastodonScheduledParams,
    pub media_attachments: Vec<MastodonMediaAttachment>,
}

#[derive(Serialize)]
pub struct MastodonScheduledParams {
    pub text: String,
    pub poll: Option<MastodonScheduledPoll>,
    pub media_ids: Option<Vec<String>>,
    pub sensitive: Option<bool>,
    pub spoiler_text: Option<String>,
    pub visibility: &'static str,
    pub in_reply_to_id: Option<String>,
    pub language: Option<String>,
    pub application_id: Option<String>,
    pub scheduled_at: Option<String>,
    pub idempotency: Option<String>,
    pub with_rate_limit: bool,
}

#[derive(Serialize)]
pub struct MastodonScheduledPoll {
    pub options: Vec<String>,
    pub expires_in: Option<String>,
    pub multiple: bool,
    pub hide_totals: bool,
}

fn poll_of(req: &CreateNoteRequest, scheduled_at: DateTime<Utc>) -> Option<MastodonScheduledPoll> {
    let p = req.poll.as_ref()?;
    // Mastodon は期限を投稿からの秒数で持つ。絶対時刻で指定された期限は予約日時からの秒数にする。
    let expires_in = p
        .expires_in_seconds
        .or_else(|| p.expired_after_ms.map(|ms| ms / 1000))
        .or_else(|| {
            p.expires_at
                .as_deref()
                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                .or_else(|| {
                    p.expires_at_epoch_ms
                        .and_then(DateTime::from_timestamp_millis)
                })
                .map(|t| (t - scheduled_at).num_seconds())
        });
    Some(MastodonScheduledPoll {
        options: p.choices.clone(),
        expires_in: expires_in.map(|s| s.to_string()),
        multiple: p.multiple.unwrap_or(false),
        hide_totals: false,
    })
}

pub(super) async fn to_scheduled_status(
    state: &AppState,
    actor_id: i64,
    row: ScheduledPostRow,
) -> Result<MastodonScheduledStatus, ApiError> {
    let req = scheduled::request_of(&row)?;
    let attachments = scheduled::attachments_of(state, actor_id, &req).await;
    let media_attachments = attachments
        .iter()
        .map(super::media::from_drive_response)
        .collect::<Result<Vec<_>, _>>()?;
    let sensitive = attachments.iter().any(|a| a.is_sensitive);
    let media_ids = req.attachment_ids.clone().filter(|ids| !ids.is_empty());
    Ok(MastodonScheduledStatus {
        id: row.id.to_string(),
        scheduled_at: mastodon_timestamp(row.scheduled_at),
        params: MastodonScheduledParams {
            text: req.text.clone().unwrap_or_default(),
            poll: poll_of(&req, row.scheduled_at),
            media_ids,
            sensitive: Some(sensitive),
            spoiler_text: req.content_warning.clone(),
            visibility: to_mastodon_visibility(req.visibility.as_deref().unwrap_or("public")),
            in_reply_to_id: req.reply_to_id.clone(),
            language: req.language.clone(),
            application_id: None,
            scheduled_at: None,
            idempotency: None,
            with_rate_limit: false,
        },
        media_attachments,
    })
}

/// `POST /api/v1/statuses` の `scheduled_at` を予約日時として読む。
pub(super) fn parse_scheduled_at(value: &str) -> Result<DateTime<Utc>, ApiError> {
    value
        .parse::<DateTime<Utc>>()
        .map_err(|_| ApiError::BadRequest("INVALID_SCHEDULED_AT".to_owned()))
}

/// `POST /api/v1/statuses` で `scheduled_at` を指定されたときの応答（`ScheduledStatus`）。
pub(super) async fn create(
    state: &AppState,
    user: &AuthedUser,
    req: CreateNoteRequest,
    scheduled_at: DateTime<Utc>,
) -> Response {
    let row = match scheduled::create(state, user, req, scheduled_at).await {
        Ok(row) => row,
        Err(e) => return e.into_response(),
    };
    match to_scheduled_status(state, user.actor_id, row).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/scheduled_statuses — 予約の新しい順（ID の降順）。
pub async fn index(
    user: AuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    MastodonQuery(page): MastodonQuery<PageParams>,
) -> Result<Response, ApiError> {
    let page = page.page(20, 40);
    let mut rows = scheduled::list(&state, &user).await?;
    rows.retain(|r| {
        page.until_id.is_none_or(|max| r.id < max) && page.since_id.is_none_or(|min| r.id > min)
    });
    rows.sort_by_key(|r| std::cmp::Reverse(r.id));
    rows.truncate(page.limit as usize);
    let cursors = match (rows.first(), rows.last()) {
        (Some(first), Some(last)) => Some((first.id, last.id)),
        _ => None,
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(to_scheduled_status(&state, user.actor_id, row).await?);
    }
    Ok(super::extract::paginated(
        &uri,
        &state.local_domain,
        out,
        cursors,
    ))
}

/// GET /api/v1/scheduled_statuses/:id
pub async fn show(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MastodonScheduledStatus>, ApiError> {
    let row = scheduled::find(&state, &user, parse_id(&id)?).await?;
    Ok(Json(to_scheduled_status(&state, user.actor_id, row).await?))
}

#[derive(Deserialize, Default)]
pub struct UpdateParams {
    pub scheduled_at: Option<String>,
}

/// PUT /api/v1/scheduled_statuses/:id — 予約日時を変える（投稿できなかった予約は再予約になる）。
pub async fn update(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    MastodonParams(params): MastodonParams<UpdateParams>,
) -> Result<Json<MastodonScheduledStatus>, ApiError> {
    let id = parse_id(&id)?;
    let scheduled_at = match params.scheduled_at.as_deref() {
        Some(v) if !v.is_empty() => parse_scheduled_at(v)?,
        _ => scheduled::find(&state, &user, id).await?.scheduled_at,
    };
    let row = scheduled::update(&state, &user, id, None, scheduled_at).await?;
    Ok(Json(to_scheduled_status(&state, user.actor_id, row).await?))
}

/// DELETE /api/v1/scheduled_statuses/:id
pub async fn destroy(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    scheduled::delete(&state, &user, parse_id(&id)?).await?;
    Ok(Json(serde_json::json!({})))
}
