use super::*;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollVoteRequest {
    option_indexes: Vec<usize>,
}

/// POST /api/notes/:id/poll-vote
pub async fn vote_poll(
    Path(note_id_str): Path<String>,
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<PollVoteRequest>,
) -> impl IntoResponse {
    let Ok(note_id) = note_id_str.parse::<i64>() else {
        return ApiError::NotFound("NOT_FOUND").into_response();
    };
    if req.option_indexes.is_empty() {
        return ApiError::BadRequest("POLL_OPTION_REQUIRED".to_owned()).into_response();
    }
    let row =
        match sqlx::query("SELECT poll, actor_id FROM posts WHERE id = $1 AND deleted_at IS NULL")
            .bind(note_id)
            .fetch_optional(&state.db)
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
            Err(e) => return ApiError::Internal(e.to_string()).into_response(),
        };
    let Some(mut poll): Option<serde_json::Value> = row.try_get("poll").unwrap_or(None) else {
        return ApiError::BadRequest("NOT_A_POLL".to_owned()).into_response();
    };
    let post_author_id: i64 = match row.try_get("actor_id") {
        Ok(id) => id,
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };
    let Some(options) = poll["options"].as_array() else {
        return ApiError::BadRequest("INVALID_POLL".to_owned()).into_response();
    };
    let multiple = poll["multiple"].as_bool().unwrap_or(false);
    let mut indexes = req.option_indexes;
    indexes.sort_unstable();
    indexes.dedup();
    if (!multiple && indexes.len() != 1) || indexes.iter().any(|i| *i >= options.len()) {
        return ApiError::BadRequest("INVALID_POLL_OPTIONS".to_owned()).into_response();
    }
    let closed = poll["closed"]
        .as_str()
        .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
        .or_else(|| {
            poll["endTime"]
                .as_str()
                .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
        })
        .is_some_and(|at| at <= chrono::Utc::now());
    if closed {
        return ApiError::BadRequest("POLL_CLOSED".to_owned()).into_response();
    }
    let existing: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM poll_votes WHERE post_id = $1 AND actor_id = $2")
            .bind(note_id)
            .bind(user.actor_id)
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
    if existing > 0 {
        return ApiError::Conflict("ALREADY_VOTED").into_response();
    }

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };
    for index in &indexes {
        if let Err(e) = sqlx::query(
            "INSERT INTO poll_votes (post_id, actor_id, option_index) VALUES ($1, $2, $3)",
        )
        .bind(note_id)
        .bind(user.actor_id)
        .bind(*index as i32)
        .execute(&mut *tx)
        .await
        {
            return ApiError::Internal(e.to_string()).into_response();
        }
    }
    if let Some(options) = poll["options"].as_array_mut() {
        for index in &indexes {
            let votes = options[*index]["votes"].as_i64().unwrap_or(0);
            options[*index]["votes"] = serde_json::json!(votes + 1);
        }
    }
    if let Err(e) = sqlx::query("UPDATE posts SET poll = $2 WHERE id = $1")
        .bind(note_id)
        .bind(&poll)
        .execute(&mut *tx)
        .await
    {
        return ApiError::Internal(e.to_string()).into_response();
    }
    if let Err(e) = tx.commit().await {
        return ApiError::Internal(e.to_string()).into_response();
    }

    // タイムライン/ノート詳細のアンケート結果をリアルタイム更新する（`broadcast_reaction_update`
    // と同じ考え方。自作自演でも送出し、他タブ・他端末の即時反映も担う）。
    broadcast_poll_update(
        &state.stream_hub,
        state.follows.as_ref(),
        note_id,
        post_author_id,
        &poll,
    )
    .await;

    let option_names = indexes
        .iter()
        .filter_map(|i| poll["options"][*i]["name"].as_str().map(str::to_owned))
        .collect();
    state
        .enqueue_ap_delivery(
            user.actor_id,
            ApDeliveryKind::PollVote {
                post_id: note_id,
                option_names,
            },
        )
        .await;
    Json(serde_json::json!({"ok": true, "poll": poll, "voted": true})).into_response()
}
