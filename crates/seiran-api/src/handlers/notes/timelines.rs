use super::*;

pub async fn home_timeline(
    Query(q): Query<TimelineQuery>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = user.actor_id;

    let limit = q.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match state
        .posts
        .home_timeline(actor_id, limit, until_id, since_id, q.exclude_direct)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[home_timeline] クエリ失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    let notes = build_note_responses(&state, rows, Some(actor_id)).await;
    Json(notes).into_response()
}

pub async fn local_timeline(
    Query(q): Query<TimelineQuery>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());
    let page = seiran_common::repository::Page {
        limit,
        until_id,
        since_id,
    };
    if q.moderation {
        return match moderation_local_timeline(&state, user.as_ref(), page).await {
            Ok(notes) => Json(notes).into_response(),
            Err(e) => e.into_response(),
        };
    }

    let my_actor_id: Option<i64> = user.map(|u| u.actor_id);
    let rows = match state
        .posts
        .local_timeline(my_actor_id, limit, until_id, since_id, q.exclude_direct)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[local_timeline] クエリ失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    let notes = build_note_responses(&state, rows, my_actor_id).await;
    Json(notes).into_response()
}

/// 管理者のモデレーション表示のローカルタイムライン。ひかえめ・プライベートを含むローカルの
/// 全投稿（DMを除く）を返し、管理者自身から本来見えない投稿には`visible_by_admin_override`を
/// 付ける。管理者以外は`ADMIN_REQUIRED`。
async fn moderation_local_timeline(
    state: &AppState,
    user: Option<&AuthedUser>,
    page: seiran_common::repository::Page,
) -> Result<Vec<NoteResponse>, ApiError> {
    let Some(authed) = user else {
        return Err(ApiError::Unauthorized("UNAUTHORIZED"));
    };
    if !super::queries::user_is_admin(state, authed.user_id).await? {
        return Err(ApiError::Forbidden("ADMIN_REQUIRED"));
    }
    let rows = state
        .posts
        .local_timeline_for_moderation(authed.actor_id, page)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut notes = build_note_responses(state, rows, Some(authed.actor_id)).await;
    super::queries::mark_admin_override_posts(state, &mut notes, Some(authed.actor_id)).await?;
    Ok(notes)
}

/// ソーシャルタイムライン（自分 + フォロー中 + ローカル全体、#78）。
pub async fn social_timeline(
    Query(q): Query<TimelineQuery>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = user.actor_id;

    let limit = q.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match state
        .posts
        .social_timeline(actor_id, limit, until_id, since_id, q.exclude_direct)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[social_timeline] クエリ失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    let notes = build_note_responses(&state, rows, Some(actor_id)).await;
    Json(notes).into_response()
}

/// グローバルタイムライン（`posts`テーブルの全投稿、#78）。
pub async fn global_timeline(
    Query(q): Query<TimelineQuery>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let my_actor_id: Option<i64> = user.map(|u| u.actor_id);

    let limit = q.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match state
        .posts
        .global_timeline(my_actor_id, limit, until_id, since_id, q.exclude_direct)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[global_timeline] クエリ失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    let notes = build_note_responses(&state, rows, my_actor_id).await;
    Json(notes).into_response()
}
