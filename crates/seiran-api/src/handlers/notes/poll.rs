use super::*;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollVoteRequest {
    /// Misskey互換API（`handlers::misskey::endpoints::notes_polls_vote`）が単一選択の
    /// `choice`から`vec![choice]`を組み立てて再利用するため`pub(crate)`にしている。
    pub(crate) option_indexes: Vec<usize>,
}

/// 投票の記録結果（配信・配送に使う）。
struct RecordedVote {
    poll: serde_json::Value,
    post_author_id: i64,
    indexes: Vec<usize>,
}

/// `option_indexes`をアンケートの設定（単一/複数選択・選択肢数・締切）に照らして検証し、
/// 重複を除いた昇順の選択肢indexを返す。
fn validate_vote(
    poll: &serde_json::Value,
    option_indexes: Vec<usize>,
) -> Result<Vec<usize>, ApiError> {
    let Some(options) = poll["options"].as_array() else {
        return Err(ApiError::BadRequest("INVALID_POLL".to_owned()));
    };
    let multiple = poll["multiple"].as_bool().unwrap_or(false);
    let mut indexes = option_indexes;
    indexes.sort_unstable();
    indexes.dedup();
    if (!multiple && indexes.len() != 1) || indexes.iter().any(|i| *i >= options.len()) {
        return Err(ApiError::BadRequest("INVALID_POLL_OPTIONS".to_owned()));
    }
    if seiran_common::repository::poll::poll_closed_at(poll)
        .is_some_and(|at| at <= chrono::Utc::now())
    {
        return Err(ApiError::BadRequest("POLL_CLOSED".to_owned()));
    }
    Ok(indexes)
}

/// 投票済み判定・`poll_votes`への記録・票数加算を1トランザクションで行う。対象ポスト行を
/// `FOR UPDATE`でロックして同一アンケートへの投票を直列化するため、同じ人の同時投票が
/// 二重に記録されることも、別の人の同時投票で票数が失われることもない。外部配送は
/// トランザクションの外（呼び出し元）で行う。
async fn record_vote(
    state: &AppState,
    note_id: i64,
    actor_id: i64,
    option_indexes: Vec<usize>,
) -> Result<RecordedVote, ApiError> {
    let internal = |e: sqlx::Error| ApiError::Internal(e.to_string());
    let mut tx = state.db.begin().await.map_err(internal)?;
    let row: Option<(Option<serde_json::Value>, i64)> = sqlx::query_as(
        "SELECT poll, actor_id FROM posts WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(note_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?;
    let (poll, post_author_id) = row.ok_or(ApiError::NotFound("NOT_FOUND"))?;
    let poll = poll.ok_or_else(|| ApiError::BadRequest("NOT_A_POLL".to_owned()))?;
    let indexes = validate_vote(&poll, option_indexes)?;

    let already_voted: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM poll_votes WHERE post_id = $1 AND actor_id = $2)",
    )
    .bind(note_id)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;
    if already_voted {
        return Err(ApiError::Conflict("ALREADY_VOTED"));
    }

    let index_values: Vec<i32> = indexes.iter().map(|i| *i as i32).collect();
    sqlx::query(
        "INSERT INTO poll_votes (post_id, actor_id, option_index)
         SELECT $1, $2, unnest($3::int[])",
    )
    .bind(note_id)
    .bind(actor_id)
    .bind(&index_values)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;
    let poll =
        seiran_common::repository::poll::increment_poll_votes(&mut tx, note_id, &index_values)
            .await
            .map_err(internal)?
            .ok_or_else(|| ApiError::BadRequest("INVALID_POLL".to_owned()))?;
    tx.commit().await.map_err(internal)?;

    Ok(RecordedVote {
        poll,
        post_author_id,
        indexes,
    })
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
    let vote = match record_vote(&state, note_id, user.actor_id, req.option_indexes).await {
        Ok(vote) => vote,
        Err(e) => return e.into_response(),
    };

    // タイムライン/ノート詳細のアンケート結果をリアルタイム更新する（`broadcast_reaction_update`
    // と同じ考え方。自作自演でも送出し、他タブ・他端末の即時反映も担う）。
    broadcast_poll_update(
        &state.stream_hub,
        state.follows.as_ref(),
        note_id,
        vote.post_author_id,
        &vote.poll,
    )
    .await;

    let option_names = vote
        .indexes
        .iter()
        .filter_map(|i| vote.poll["options"][*i]["name"].as_str().map(str::to_owned))
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
    Json(serde_json::json!({"ok": true, "poll": vote.poll, "voted": true})).into_response()
}
