//! Misskey の予約投稿（`notes/drafts/*`）。Misskey は予約投稿を「`scheduledAt` と
//! `isActuallyScheduled` の付いた下書き」として扱う。seiran はサーバー側の下書きを持たないので、
//! 予約（`isActuallyScheduled: true`）だけを受け付け、投稿できなかった予約を
//! `isActuallyScheduled: false` の下書きとして返す（Misskey でも、投稿できなかった予約は予約の
//! 外れた下書きとして残る）。検証・保存・実行は `handlers::scheduled_posts` と共有する。

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::handlers::drive::DriveFileResponse;
use crate::handlers::notes::dto::CreateNoteRequest;
use crate::handlers::scheduled_posts::{self as scheduled, ScheduledPostRow};
use crate::middleware::AuthedUser;
use crate::AppState;

use super::convert::{build_notes, to_misskey_visibility, user_lite, ActorSummary};
use super::endpoints::CursorParams;
use super::types::{MisskeyNote, MisskeyUserLite};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyNoteDraft {
    pub id: String,
    pub created_at: String,
    pub text: Option<String>,
    pub cw: Option<String>,
    pub user_id: String,
    pub user: MisskeyUserLite,
    pub reply_id: Option<String>,
    pub renote_id: Option<String>,
    pub reply: Option<MisskeyNote>,
    pub renote: Option<MisskeyNote>,
    pub visibility: String,
    pub visible_user_ids: Vec<String>,
    pub file_ids: Vec<String>,
    pub files: Vec<DriveFileResponse>,
    pub hashtag: Option<String>,
    pub poll: Option<MisskeyDraftPoll>,
    pub channel_id: Option<String>,
    pub channel: Option<serde_json::Value>,
    pub local_only: bool,
    pub reaction_acceptance: Option<String>,
    pub scheduled_at: Option<i64>,
    pub is_actually_scheduled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyDraftPoll {
    pub expires_at: Option<String>,
    pub expired_after: Option<i64>,
    pub multiple: bool,
    pub choices: Vec<String>,
}

fn poll_of(req: &CreateNoteRequest) -> Option<MisskeyDraftPoll> {
    let p = req.poll.as_ref()?;
    let expires_at = p
        .expires_at
        .as_deref()
        .and_then(|s| s.parse::<DateTime<chrono::Utc>>().ok())
        .or_else(|| {
            p.expires_at_epoch_ms
                .and_then(DateTime::from_timestamp_millis)
        });
    Some(MisskeyDraftPoll {
        expires_at: expires_at.map(|t| t.to_rfc3339()),
        expired_after: p
            .expired_after_ms
            .or_else(|| p.expires_in_seconds.map(|s| s * 1000)),
        multiple: p.multiple.unwrap_or(false),
        choices: p.choices.clone(),
    })
}

async fn to_draft(
    state: &AppState,
    user: &AuthedUser,
    row: ScheduledPostRow,
) -> Result<MisskeyNoteDraft, ApiError> {
    let req = scheduled::request_of(&row)?;
    let (files, (reply, quote)) = tokio::join!(
        scheduled::attachments_of(state, user.actor_id, &req),
        scheduled::referenced_posts(state, user.actor_id, &req)
    );
    let posts: Vec<_> = reply.into_iter().chain(quote).collect();
    let mut notes = build_notes(state, posts, Some(user.actor_id)).await;
    let mut take = |id: &Option<String>| {
        let pos = notes.iter().position(|n| Some(&n.id) == id.as_ref())?;
        Some(notes.remove(pos))
    };
    let reply = take(&req.reply_to_id);
    let renote = take(&req.quote_of_id);
    let avatar_url = state
        .actors
        .find_avatar_url(user.actor_id)
        .await
        .ok()
        .flatten();
    let user_lite = user_lite(
        ActorSummary {
            id: user.actor_id,
            username: &user.username,
            domain: &user.domain,
            actor_type: "local",
            display_name: user.display_name.as_deref(),
            avatar_url: avatar_url.as_deref(),
        },
        &state.local_domain,
    );
    Ok(MisskeyNoteDraft {
        id: row.id.to_string(),
        created_at: row.created_at.to_rfc3339(),
        text: req.text.clone(),
        cw: req.content_warning.clone(),
        user_id: user.actor_id.to_string(),
        user: user_lite,
        reply_id: req.reply_to_id.clone(),
        renote_id: req.quote_of_id.clone(),
        reply,
        renote,
        visibility: to_misskey_visibility(req.visibility.as_deref().unwrap_or("public")),
        visible_user_ids: req.recipient_actor_ids.clone().unwrap_or_default(),
        file_ids: req.attachment_ids.clone().unwrap_or_default(),
        files,
        hashtag: None,
        poll: poll_of(&req),
        channel_id: None,
        channel: None,
        local_only: false,
        reaction_acceptance: None,
        scheduled_at: Some(row.scheduled_at.timestamp_millis()),
        is_actually_scheduled: row.failure_code.is_none(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftBody {
    pub draft_id: Option<String>,
    /// 予約日時（epoch ミリ秒）。
    pub scheduled_at: Option<i64>,
    #[serde(default)]
    pub is_actually_scheduled: bool,
    #[serde(flatten)]
    pub request: CreateNoteRequest,
}

impl DraftBody {
    /// 予約日時。seiran は予約でない下書きを持てないので、予約でなければ拒む。
    fn scheduled_at(&self) -> Result<DateTime<chrono::Utc>, ApiError> {
        if !self.is_actually_scheduled {
            return Err(ApiError::BadRequest(
                "UNSCHEDULED_DRAFT_NOT_SUPPORTED".to_owned(),
            ));
        }
        self.scheduled_at
            .and_then(DateTime::from_timestamp_millis)
            .ok_or_else(|| ApiError::BadRequest("INVALID_SCHEDULED_AT".to_owned()))
    }
}

fn parse_draft_id(id: Option<&str>) -> Result<i64, ApiError> {
    id.and_then(|s| s.parse().ok())
        .ok_or(ApiError::NotFound("NO_SUCH_NOTE_DRAFT"))
}

/// POST /api/notes/drafts/create
pub async fn create(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<DraftBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scheduled_at = body.scheduled_at()?;
    let row = scheduled::create(&state, &user, body.request, scheduled_at).await?;
    let draft = to_draft(&state, &user, row).await?;
    Ok(Json(serde_json::json!({ "createdDraft": draft })))
}

/// POST /api/notes/drafts/update — 内容と予約日時を書き換える（投稿できなかった予約の再予約）。
pub async fn update(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<DraftBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_draft_id(body.draft_id.as_deref())?;
    let scheduled_at = body.scheduled_at()?;
    let row = scheduled::update(&state, &user, id, Some(body.request), scheduled_at).await?;
    let draft = to_draft(&state, &user, row).await?;
    Ok(Json(serde_json::json!({ "updatedDraft": draft })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftIdBody {
    pub draft_id: String,
}

/// POST /api/notes/drafts/delete
pub async fn delete(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<DraftIdBody>,
) -> Result<impl IntoResponse, ApiError> {
    let id = parse_draft_id(Some(&body.draft_id))?;
    scheduled::delete(&state, &user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
pub struct DraftListBody {
    /// `true` なら投稿待ちの予約だけ、`false` なら投稿できなかった予約だけ。
    pub scheduled: Option<bool>,
    #[serde(flatten)]
    pub cursor: CursorParams,
}

/// POST /api/notes/drafts/list — 新しい順（ID の降順）。
pub async fn list(
    user: AuthedUser,
    State(state): State<AppState>,
    body: Option<Json<DraftListBody>>,
) -> Result<Json<Vec<MisskeyNoteDraft>>, ApiError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let page = body.cursor.page(30);
    let mut rows = scheduled::list(&state, &user).await?;
    rows.retain(|r| {
        body.scheduled
            .is_none_or(|scheduled| scheduled == r.failure_code.is_none())
            && page.until_id.is_none_or(|max| r.id < max)
            && page.since_id.is_none_or(|min| r.id > min)
    });
    rows.sort_by_key(|r| std::cmp::Reverse(r.id));
    rows.truncate(page.limit as usize);
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(to_draft(&state, &user, row).await?);
    }
    Ok(Json(out))
}

/// POST /api/notes/drafts/count
pub async fn count(user: AuthedUser, State(state): State<AppState>) -> Result<Json<i64>, ApiError> {
    Ok(Json(scheduled::list(&state, &user).await?.len() as i64))
}
