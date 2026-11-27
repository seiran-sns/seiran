//! 予約投稿（タイマー投稿）。
//!
//! 予約は投稿リクエスト（[`CreateNoteRequest`]）をそのまま `scheduled_posts` に保存し、予約日時に
//! 通常の投稿処理（`notes::creation::publish_scheduled_post`）へ流す。受け付け時にも同じ検証を
//! 予約日時を「今」として通し、明らかに投稿できない内容はその場で拒む。
//!
//! - 二重投稿: 投稿の INSERT が `posts.scheduled_post_id` の一意制約に拒まれた側は、出遅れた
//!   ことを知って何もしない（複数のプロセスで実行ループが動いてよい）。
//! - 投稿漏れ: 予約行の削除は投稿の INSERT と同じトランザクションで行う。
//! - 遅れ: 予約日時から [`LATE_LIMIT`] を過ぎても投稿できていない予約は、場違いな時期に
//!   投稿しないよう「投稿できなかった予約」にする（予約一覧から編集・再予約・削除できる）。
//! - 投稿数のレート制限に達していれば、予約は投稿待ちのまま、制限が解けた後の周回で投稿する。
//! - 凍結中のアカウントの予約は消さず、実行だけを見送る。
//!
//! seiran UI 用の `/api/scheduled-posts`、Mastodon の `scheduled_statuses`、Misskey の
//! `notes/drafts/*` は、ここの共通関数を呼んでレスポンスの形だけを変える。

use std::collections::HashSet;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use seiran_common::edit_diff::{merge_object, Edit, EditConflict};
use seiran_common::generate_snowflake_id;
use seiran_common::repository::scheduled_post::{self as repo, ScheduledPostCounts};

pub use seiran_common::repository::scheduled_post::ScheduledPostRow;

use crate::error::ApiError;
use crate::handlers::drive::DriveFileResponse;
use crate::handlers::notes::dto::CreateNoteRequest;
use crate::handlers::notes::NoteResponse;
use crate::middleware::AuthedUser;
use crate::AppState;

/// 予約日時は今からこれ以上先でなければならない（受け付けた直後に実行ループが拾うような
/// 予約は、即時投稿と区別が付かず予約の意味が無い）。予約日時を変えない編集には適用しない。
pub const MIN_LEAD: Duration = Duration::minutes(5);
/// 予約日時はこれより先にできない。
pub const MAX_AHEAD: Duration = Duration::days(365);
/// 予約日時からこれ以上遅れたら投稿せず「投稿できなかった予約」にする。
pub const LATE_LIMIT: Duration = Duration::hours(24);
/// 遅れて投稿できなかった予約のエラーコード。
pub const TOO_LATE: &str = "SCHEDULED_POST_TOO_LATE";
/// 実行ループの間隔。
const TICK_SECS: u64 = 15;
/// 1周回で処理する予約の上限。
const TICK_BATCH: i64 = 50;

/// 予約の作成・件数変化を本人の他のタブへ知らせるストリーミングイベント。
const CHANGED_EVENT: &str = "scheduledPostsChanged";

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

/// クライアントへ返し、`failure_code` として残すエラーコード。
fn error_code(e: &ApiError) -> String {
    match e {
        ApiError::Unauthorized(c)
        | ApiError::NotFound(c)
        | ApiError::Conflict(c)
        | ApiError::Forbidden(c)
        | ApiError::ServiceUnavailable(c)
        | ApiError::TooManyRequests(c, _) => (*c).to_owned(),
        ApiError::BadRequest(c) | ApiError::BadGateway(c) => c.clone(),
        ApiError::InsufficientStorage => "STORAGE_QUOTA_EXCEEDED".to_owned(),
        ApiError::Internal(_) => "INTERNAL_ERROR".to_owned(),
    }
}

fn notify_changed(state: &AppState, actor_id: i64) {
    state.stream_hub.publish_event(
        HashSet::from([actor_id]),
        CHANGED_EVENT,
        serde_json::json!({}),
    );
}

fn parse_id(id: &str) -> Result<i64, ApiError> {
    id.parse()
        .map_err(|_| ApiError::NotFound("SCHEDULED_POST_NOT_FOUND"))
}

/// 予約日時を検証する。`current` は編集前の予約日時（変えない編集は近すぎても通す）。
fn validate_scheduled_at(
    scheduled_at: DateTime<Utc>,
    current: Option<DateTime<Utc>>,
) -> Result<(), ApiError> {
    let now = Utc::now();
    if current == Some(scheduled_at) && scheduled_at > now {
        return Ok(());
    }
    if scheduled_at < now + MIN_LEAD {
        return Err(ApiError::BadRequest("SCHEDULED_AT_TOO_SOON".to_owned()));
    }
    if scheduled_at > now + MAX_AHEAD {
        return Err(ApiError::BadRequest("SCHEDULED_AT_TOO_FAR".to_owned()));
    }
    Ok(())
}

/// 保存されている予約のリクエストを読み戻す。
pub fn request_of(row: &ScheduledPostRow) -> Result<CreateNoteRequest, ApiError> {
    serde_json::from_value(row.request.clone()).map_err(|e| {
        internal(format!(
            "予約投稿のリクエストを読めない id={}: {}",
            row.id, e
        ))
    })
}

/// リクエストを予約として保存できる形に正規化・検証する。公開範囲は seiran の語彙にして
/// 保存する（Misskey・Mastodon の形で読み戻すとき、語彙を変換し直せるように）。
async fn prepare_request(
    state: &AppState,
    actor_id: i64,
    mut req: CreateNoteRequest,
    scheduled_at: DateTime<Utc>,
) -> Result<serde_json::Value, ApiError> {
    normalize(&mut req);
    crate::handlers::notes::creation::validate_scheduled_request(
        state,
        actor_id,
        &req,
        scheduled_at,
    )
    .await?;
    serde_json::to_value(&req).map_err(internal)
}

/// 保存する形への正規化（検証はしない）。差分を突き合わせるときも同じ形にそろえる。
fn normalize(req: &mut CreateNoteRequest) {
    req.normalize_misskey_quote();
    req.visibility = req
        .visibility
        .take()
        .map(|v| crate::handlers::notes::delivery::normalize_misskey_visibility(&v).to_owned());
}

/// 予約の編集画面を開いた時点の予約（`ScheduledPostResponse` の `scheduledAt`・`request`）。
/// 保存はここからの差分として、別の画面での編集と合わせる（`seiran_common::edit_diff`）。
#[derive(Deserialize)]
pub struct ScheduleBase {
    pub scheduled_at: DateTime<Utc>,
    pub request: CreateNoteRequest,
}

/// 本文と CW は行単位の 3-way マージ、それ以外の項目は値として当てる。
const TEXT_KEYS: &[&str] = &["text", "content_warning"];

fn request_object(
    mut req: CreateNoteRequest,
) -> Result<serde_json::Map<String, serde_json::Value>, ApiError> {
    normalize(&mut req);
    match serde_json::to_value(req).map_err(internal)? {
        serde_json::Value::Object(map) => Ok(map),
        _ => Err(internal("予約投稿のリクエストがオブジェクトでない")),
    }
}

/// 編集の差分を今の予約に当てた結果。
struct MergedEdit {
    request: CreateNoteRequest,
    scheduled_at: DateTime<Utc>,
    /// 別の画面での編集も取り込んだ（編集後の内容そのままではない）。
    combined: bool,
}

/// 編集画面を開いた時点 `base` からの編集（`edited`・`scheduled_at`）を、今の予約 `current` に
/// 当てる。
fn merge_onto(
    current: &ScheduledPostRow,
    base: &serde_json::Map<String, serde_json::Value>,
    base_scheduled_at: DateTime<Utc>,
    edited: &serde_json::Map<String, serde_json::Value>,
    scheduled_at: DateTime<Utc>,
) -> Result<MergedEdit, ApiError> {
    let current_request = request_object(request_of(current)?)?;
    let request = merge_object(base, edited, &current_request, TEXT_KEYS)?;
    let merged_at = Edit::Change {
        from: base_scheduled_at,
        to: scheduled_at,
    }
    .apply_value(&current.scheduled_at)?;
    let combined = request != *edited || merged_at != scheduled_at;
    Ok(MergedEdit {
        request: serde_json::from_value(serde_json::Value::Object(request))
            .map_err(|_| EditConflict)?,
        scheduled_at: merged_at,
        combined,
    })
}

/// 編集後の内容のままなら通る検証に、別の画面の編集と合わせたせいで落ちたら（本文の上限超過
/// 等）、どちらかを直さないと保存できないので衝突として知らせる。
fn conflict_if_combined(merged: &MergedEdit, e: ApiError) -> ApiError {
    match e {
        ApiError::BadRequest(_) if merged.combined => EditConflict.into(),
        e => e,
    }
}

/// 読み直してから書くまでに別の画面が保存したときに、読み直して当て直す回数。
const MERGE_ATTEMPTS: usize = 3;

/// 予約の編集画面からの保存。開いた時点 `base` からの差分を今の予約に当てて、内容と予約日時を
/// 書き換えて投稿待ちに戻す。別の画面での編集と重なったら何も変えず `EDIT_CONFLICT`。
///
/// 検証（返信先の確認等）にはリモートへの問い合わせを含むので、行をロックしたまま待たずに、
/// 読んだ時点の `revision` のままなら書く1文の `UPDATE` で保存し、その間に書き換えられて
/// いたら読み直して当て直す。
pub async fn update_from_base(
    state: &AppState,
    user: &AuthedUser,
    id: i64,
    req: CreateNoteRequest,
    scheduled_at: DateTime<Utc>,
    base: ScheduleBase,
) -> Result<ScheduledPostRow, ApiError> {
    user.require_not_did_moved_out()?;
    let base_request = request_object(base.request)?;
    let edited = request_object(req)?;
    for _ in 0..MERGE_ATTEMPTS {
        let current = find(state, user, id).await?;
        let merged = merge_onto(
            &current,
            &base_request,
            base.scheduled_at,
            &edited,
            scheduled_at,
        )?;
        validate_scheduled_at(merged.scheduled_at, Some(current.scheduled_at))
            .map_err(|e| conflict_if_combined(&merged, e))?;
        let request = prepare_request(
            state,
            user.actor_id,
            merged.request.clone(),
            merged.scheduled_at,
        )
        .await
        .map_err(|e| conflict_if_combined(&merged, e))?;
        if let Some(row) = repo::update_if_revision(
            &state.db,
            id,
            user.actor_id,
            &request,
            merged.scheduled_at,
            current.revision,
        )
        .await
        .map_err(internal)?
        {
            notify_changed(state, user.actor_id);
            return Ok(row);
        }
    }
    Err(EditConflict.into())
}

/// 予約を作る。
pub async fn create(
    state: &AppState,
    user: &AuthedUser,
    req: CreateNoteRequest,
    scheduled_at: DateTime<Utc>,
) -> Result<ScheduledPostRow, ApiError> {
    user.require_not_did_moved_out()?;
    validate_scheduled_at(scheduled_at, None)?;
    let request = prepare_request(state, user.actor_id, req, scheduled_at).await?;
    let max = crate::rate_limit::scheduled_post_max(state, user.actor_id).await;
    let row = repo::insert_if_under_limit(
        &state.db,
        generate_snowflake_id(Utc::now()),
        user.actor_id,
        &request,
        scheduled_at,
        max,
    )
    .await
    .map_err(internal)?
    .ok_or(ApiError::Conflict("TOO_MANY_SCHEDULED_POSTS"))?;
    notify_changed(state, user.actor_id);
    Ok(row)
}

/// 予約を書き換えて投稿待ちに戻す（投稿できなかった予約の再予約を兼ねる）。`req` が `None` なら
/// 予約日時だけを変える（保存済みの内容を新しい予約日時で検証し直す）。
pub async fn update(
    state: &AppState,
    user: &AuthedUser,
    id: i64,
    req: Option<CreateNoteRequest>,
    scheduled_at: DateTime<Utc>,
) -> Result<ScheduledPostRow, ApiError> {
    user.require_not_did_moved_out()?;
    let current = find(state, user, id).await?;
    validate_scheduled_at(scheduled_at, Some(current.scheduled_at))?;
    let req = match req {
        Some(req) => req,
        None => request_of(&current)?,
    };
    let request = prepare_request(state, user.actor_id, req, scheduled_at).await?;
    let row = repo::update(&state.db, id, user.actor_id, Some(&request), scheduled_at)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("SCHEDULED_POST_NOT_FOUND"))?;
    notify_changed(state, user.actor_id);
    Ok(row)
}

/// 予約を取り消す。
pub async fn delete(state: &AppState, user: &AuthedUser, id: i64) -> Result<(), ApiError> {
    if !repo::delete(&state.db, id, user.actor_id)
        .await
        .map_err(internal)?
    {
        return Err(ApiError::NotFound("SCHEDULED_POST_NOT_FOUND"));
    }
    notify_changed(state, user.actor_id);
    Ok(())
}

pub async fn find(
    state: &AppState,
    user: &AuthedUser,
    id: i64,
) -> Result<ScheduledPostRow, ApiError> {
    repo::find_for_actor(&state.db, id, user.actor_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("SCHEDULED_POST_NOT_FOUND"))
}

pub async fn list(state: &AppState, user: &AuthedUser) -> Result<Vec<ScheduledPostRow>, ApiError> {
    repo::list_by_actor(&state.db, user.actor_id)
        .await
        .map_err(internal)
}

/// 予約を今すぐ投稿する（`req` で内容を差し替えられる）。予約の実行と同じ経路を通るので、
/// 実行ループと同時に動いても二重には投稿されない。
pub async fn publish_now(
    state: &AppState,
    user: &AuthedUser,
    id: i64,
    req: Option<CreateNoteRequest>,
    base: Option<CreateNoteRequest>,
) -> Result<NoteResponse, ApiError> {
    user.require_not_did_moved_out()?;
    let row = find(state, user, id).await?;
    // 編集画面を開いた時点 `base` があれば、そこからの差分を今の予約に当てて投稿する（別の画面
    // での編集を落とさない）。読んでから投稿するまでに書き換えられたら投稿しない（下の
    // SCHEDULED_POST_CHANGED）
    let mut merged = None;
    let mut req = match (req, base) {
        (Some(req), Some(base)) => {
            let edit = merge_onto(
                &row,
                &request_object(base)?,
                row.scheduled_at,
                &request_object(req)?,
                row.scheduled_at,
            )?;
            let req = edit.request.clone();
            merged = Some(edit);
            req
        }
        (Some(req), None) => req,
        (None, _) => request_of(&row)?,
    };
    req.normalize_misskey_quote();
    let author = state
        .actors
        .find_by_id(user.actor_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("ACTOR_NOT_FOUND"))?;
    let note = crate::handlers::notes::creation::publish_scheduled_post(
        state,
        &author,
        &req,
        row.source(),
    )
    .await
    .map_err(|e| match e {
        // 実行ループが先に投稿した、または別のタブで編集・削除された。
        ApiError::Conflict("SCHEDULED_POST_ALREADY_HANDLED") => {
            ApiError::Conflict("SCHEDULED_POST_CHANGED")
        }
        e => match &merged {
            Some(merged) => conflict_if_combined(merged, e),
            None => e,
        },
    })?;
    notify_changed(state, user.actor_id);
    Ok(note)
}

/// 予約の添付（ドライブのファイル）。NSFW 指定は予約の `sensitive_attachment_ids`、無ければ
/// ドライブで付けた指定を写す（投稿時と同じ）。ドライブから消えたファイルは含めない。
pub async fn attachments_of(
    state: &AppState,
    actor_id: i64,
    req: &CreateNoteRequest,
) -> Vec<DriveFileResponse> {
    let ids: Vec<i64> = req
        .attachment_ids
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let sensitive: Vec<i64> = match &req.sensitive_attachment_ids {
        Some(ids) => ids.iter().filter_map(|s| s.parse().ok()).collect(),
        None => {
            seiran_common::repository::media_file::drive_sensitive_ids(&state.db, actor_id, &ids)
                .await
                .unwrap_or_default()
        }
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(Some(record)) = state.media_files.find_by_id(id).await {
            out.push(
                crate::handlers::drive::to_drive_file_response(
                    state,
                    &record,
                    sensitive.contains(&id),
                )
                .await,
            );
        }
    }
    out
}

fn parse_post_id(id: &Option<String>) -> Option<i64> {
    id.as_deref().and_then(|s| s.parse().ok())
}

/// 予約の返信先・引用元の投稿（閲覧できなくなっていれば `None`）。
pub async fn referenced_posts(
    state: &AppState,
    actor_id: i64,
    req: &CreateNoteRequest,
) -> (
    Option<seiran_common::repository::TimelinePost>,
    Option<seiran_common::repository::TimelinePost>,
) {
    let find = |id: Option<i64>| async move {
        match id {
            Some(id) => state
                .posts
                .find_by_id_for_viewer(id, Some(actor_id))
                .await
                .ok()
                .flatten(),
            None => None,
        }
    };
    tokio::join!(
        find(parse_post_id(&req.reply_to_id)),
        find(parse_post_id(&req.quote_of_id))
    )
}

// =====================================================================
// 実行ループ
// =====================================================================

/// 予約投稿の実行ループを起動する。API を受けるプロセスごとに動かしてよい（同じ予約を
/// 同時に実行しても、投稿の一意制約で片方だけが投稿する）。
pub fn spawn_scheduled_post_task(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            run_due_scheduled_posts(&state).await;
        }
    });
}

async fn run_due_scheduled_posts(state: &AppState) {
    let now = Utc::now();
    let due = match repo::list_due(&state.db, now, TICK_BATCH).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[scheduled-post] 実行対象の取得に失敗: {}", e);
            return;
        }
    };
    for row in due {
        run_scheduled_post(state, &row, now).await;
    }
}

/// 予約1件を実行した結果。
#[derive(Debug, PartialEq, Eq)]
enum RunOutcome {
    Posted,
    /// 他のプロセス・即時投稿が先に処理した、または実行中に編集・削除された。
    AlreadyHandled,
    /// 投稿数のレート制限・一時的な障害。投稿待ちのまま次の周回で試し直す。
    Retry,
    Failed(String),
}

/// 実行の失敗のうち、時間が経てば通りうるもの（投稿待ちのまま残す）か。
fn classify_error(e: &ApiError) -> RunOutcome {
    match e {
        ApiError::Conflict("SCHEDULED_POST_ALREADY_HANDLED") => RunOutcome::AlreadyHandled,
        ApiError::TooManyRequests(..)
        | ApiError::Internal(_)
        | ApiError::ServiceUnavailable(_)
        | ApiError::BadGateway(_) => RunOutcome::Retry,
        e => RunOutcome::Failed(error_code(e)),
    }
}

async fn run_scheduled_post(state: &AppState, row: &ScheduledPostRow, now: DateTime<Utc>) {
    let outcome = if row.scheduled_at + LATE_LIMIT < now {
        RunOutcome::Failed(TOO_LATE.to_owned())
    } else {
        try_publish(state, row).await
    };
    match outcome {
        RunOutcome::Posted => {
            tracing::info!("[scheduled-post] 投稿しました id={}", row.id);
            notify_changed(state, row.actor_id);
        }
        RunOutcome::AlreadyHandled => {}
        RunOutcome::Retry => {}
        RunOutcome::Failed(code) => {
            tracing::info!("[scheduled-post] 投稿できません id={}: {}", row.id, code);
            match repo::mark_failed(&state.db, row.source(), &code).await {
                Ok(true) => notify_changed(state, row.actor_id),
                Ok(false) => {}
                Err(e) => tracing::error!("[scheduled-post] 失敗の記録に失敗 id={}: {}", row.id, e),
            }
        }
    }
}

async fn try_publish(state: &AppState, row: &ScheduledPostRow) -> RunOutcome {
    let req = match request_of(row) {
        Ok(req) => req,
        Err(e) => {
            tracing::error!("{}", e);
            return RunOutcome::Failed("INVALID_SCHEDULED_REQUEST".to_owned());
        }
    };
    let author = match state.actors.find_by_id(row.actor_id).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return RunOutcome::AlreadyHandled,
        Err(e) => {
            tracing::error!("[scheduled-post] 投稿者の取得に失敗 id={}: {}", row.id, e);
            return RunOutcome::Retry;
        }
    };
    if author.did_moved_out_at.is_some() {
        return RunOutcome::Failed("DID_MOVED_OUT".to_owned());
    }
    match crate::handlers::notes::creation::publish_scheduled_post(
        state,
        &author,
        &req,
        row.source(),
    )
    .await
    {
        Ok(_) => RunOutcome::Posted,
        Err(e) => {
            let outcome = classify_error(&e);
            if let (RunOutcome::Retry, ApiError::Internal(msg)) = (&outcome, &e) {
                tracing::error!("[scheduled-post] 投稿に失敗 id={}: {}", row.id, msg);
            }
            outcome
        }
    }
}

// =====================================================================
// seiran UI 用 API（`/api/scheduled-posts`）
// =====================================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledPostResponse {
    pub id: String,
    pub scheduled_at: String,
    pub created_at: String,
    /// 投稿できなかった予約なら、その理由のエラーコード。
    pub failure_code: Option<String>,
    pub failed_at: Option<String>,
    /// 予約した投稿リクエスト（投稿 API と同じ形）。
    pub request: CreateNoteRequest,
    pub attachments: Vec<DriveFileResponse>,
    pub reply_to: Option<NoteResponse>,
    pub quote_of: Option<NoteResponse>,
}

async fn to_response(
    state: &AppState,
    actor_id: i64,
    row: ScheduledPostRow,
) -> Result<ScheduledPostResponse, ApiError> {
    let request = request_of(&row)?;
    let (attachments, (reply_to, quote_of)) = tokio::join!(
        attachments_of(state, actor_id, &request),
        referenced_posts(state, actor_id, &request)
    );
    let posts: Vec<_> = reply_to.into_iter().chain(quote_of).collect();
    let mut notes =
        crate::handlers::notes::build_note_responses(state, posts, Some(actor_id)).await;
    let mut take = |id: &Option<String>| {
        let pos = notes.iter().position(|n| Some(&n.id) == id.as_ref())?;
        Some(notes.remove(pos))
    };
    let reply_to = take(&request.reply_to_id);
    let quote_of = take(&request.quote_of_id);
    Ok(ScheduledPostResponse {
        id: row.id.to_string(),
        scheduled_at: row.scheduled_at.to_rfc3339(),
        created_at: row.created_at.to_rfc3339(),
        failure_code: row.failure_code,
        failed_at: row.failed_at.map(|t| t.to_rfc3339()),
        request,
        attachments,
        reply_to,
        quote_of,
    })
}

#[derive(Deserialize)]
pub struct ScheduleRequest {
    pub scheduled_at: DateTime<Utc>,
    /// 予約の編集画面を開いた時点の予約。あれば差分として保存する（`update_from_base`）。
    /// 無ければ上書き（差分を送らない版の画面）。
    #[serde(default)]
    pub base: Option<ScheduleBase>,
    #[serde(flatten)]
    pub request: CreateNoteRequest,
}

#[derive(Deserialize)]
pub struct PublishRequest {
    /// 予約の編集画面を開いた時点の内容。あれば差分として今の予約に当ててから投稿する。
    #[serde(default)]
    pub base: Option<CreateNoteRequest>,
    #[serde(flatten)]
    pub request: CreateNoteRequest,
}

/// GET /api/scheduled-posts — 本人の予約（投稿できなかったものを含む、予約日時の早い順）。
pub async fn list_handler(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<ScheduledPostResponse>>, ApiError> {
    let rows = list(&state, &user).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(to_response(&state, user.actor_id, row).await?);
    }
    Ok(Json(out))
}

#[derive(Serialize)]
pub struct CountsResponse {
    pub pending: i64,
    pub failed: i64,
}

/// GET /api/scheduled-posts/counts — メニューのバッジ用の件数。
pub async fn counts_handler(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<CountsResponse>, ApiError> {
    let ScheduledPostCounts { pending, failed } = repo::counts(&state.db, user.actor_id)
        .await
        .map_err(internal)?;
    Ok(Json(CountsResponse { pending, failed }))
}

/// POST /api/scheduled-posts — 予約する（本文は投稿 API と同じ、加えて `scheduled_at`）。
pub async fn create_handler(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<ScheduleRequest>,
) -> Result<Json<ScheduledPostResponse>, ApiError> {
    let row = create(&state, &user, body.request, body.scheduled_at).await?;
    Ok(Json(to_response(&state, user.actor_id, row).await?))
}

/// PUT /api/scheduled-posts/:id — 内容と予約日時を書き換えて投稿待ちに戻す。
pub async fn update_handler(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ScheduleRequest>,
) -> Result<Json<ScheduledPostResponse>, ApiError> {
    let id = parse_id(&id)?;
    let row = match body.base {
        Some(base) => {
            update_from_base(&state, &user, id, body.request, body.scheduled_at, base).await?
        }
        None => update(&state, &user, id, Some(body.request), body.scheduled_at).await?,
    };
    Ok(Json(to_response(&state, user.actor_id, row).await?))
}

/// DELETE /api/scheduled-posts/:id
pub async fn delete_handler(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    match delete(&state, &user, id).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// POST /api/scheduled-posts/:id/publish — 予約を（本文で渡した内容で）今すぐ投稿する。
pub async fn publish_handler(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PublishRequest>,
) -> Result<Json<NoteResponse>, ApiError> {
    let id = parse_id(&id)?;
    Ok(Json(
        publish_now(&state, &user, id, Some(body.request), body.base).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduled_at_must_be_ahead_by_min_lead() {
        let now = Utc::now();
        assert!(validate_scheduled_at(now + Duration::minutes(1), None).is_err());
        assert!(validate_scheduled_at(now + Duration::minutes(6), None).is_ok());
        assert!(validate_scheduled_at(now + Duration::days(366), None).is_err());
    }

    #[test]
    fn unchanged_scheduled_at_may_be_near_but_not_past() {
        let near = Utc::now() + Duration::minutes(1);
        assert!(validate_scheduled_at(near, Some(near)).is_ok());
        let past = Utc::now() - Duration::minutes(1);
        assert!(validate_scheduled_at(past, Some(past)).is_err());
    }

    #[test]
    fn rate_limits_and_transient_errors_are_retried() {
        assert_eq!(
            classify_error(&ApiError::TooManyRequests("POST_RATE_LIMITED", Some(60))),
            RunOutcome::Retry
        );
        assert_eq!(
            classify_error(&ApiError::Internal("db".to_owned())),
            RunOutcome::Retry
        );
        assert_eq!(
            classify_error(&ApiError::Conflict("SCHEDULED_POST_ALREADY_HANDLED")),
            RunOutcome::AlreadyHandled
        );
        assert_eq!(
            classify_error(&ApiError::NotFound("REPLY_TARGET_NOT_FOUND")),
            RunOutcome::Failed("REPLY_TARGET_NOT_FOUND".to_owned())
        );
    }

    #[test]
    fn stored_request_round_trips_through_json() {
        let req: CreateNoteRequest = serde_json::from_value(serde_json::json!({
            "text": "hello",
            "fileIds": ["1", "2"],
            "cw": "warn",
            "poll": { "choices": ["a", "b"], "expiredAfter": 60000 },
            "bsky_embed_choice": { "kind": "attachment", "id": "1" },
        }))
        .unwrap();
        let stored = serde_json::to_value(&req).unwrap();
        let back: CreateNoteRequest = serde_json::from_value(stored).unwrap();
        assert_eq!(back.text.as_deref(), Some("hello"));
        assert_eq!(
            back.attachment_ids,
            Some(vec!["1".to_owned(), "2".to_owned()])
        );
        assert_eq!(back.content_warning.as_deref(), Some("warn"));
        assert_eq!(back.poll.unwrap().expired_after_ms, Some(60000));
        assert_eq!(
            back.bsky_embed_choice,
            Some(crate::handlers::notes::dto::BskyEmbedChoice::Attachment { id: "1".to_owned() })
        );
    }
}
