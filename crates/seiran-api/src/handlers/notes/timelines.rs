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
    let my_actor_id: Option<i64> = user.map(|u| u.actor_id);

    let limit = q.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

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
