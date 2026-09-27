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

/// 投票を記録する（排他と加算は `repository::poll::record_local_vote`）。外部配送は呼び出し元が
/// トランザクションの外で行う。
async fn record_vote(
    state: &AppState,
    note_id: i64,
    actor_id: i64,
    option_indexes: Vec<usize>,
) -> Result<RecordedVote, ApiError> {
    use seiran_common::repository::poll::{record_local_vote, LocalVoteRejected};
    let recorded = record_local_vote(&state.db, note_id, actor_id, |poll| {
        let indexes = validate_vote(poll, option_indexes)?;
        let values = indexes.iter().map(|i| *i as i32).collect();
        Ok::<_, ApiError>((values, indexes))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    match recorded {
        Ok(v) => Ok(RecordedVote {
            poll: v.poll,
            post_author_id: v.post_author_id,
            indexes: v.choice,
        }),
        Err(LocalVoteRejected::NotFound) => Err(ApiError::NotFound("NOT_FOUND")),
        Err(LocalVoteRejected::NotAPoll) => Err(ApiError::BadRequest("NOT_A_POLL".to_owned())),
        Err(LocalVoteRejected::Invalid(e)) => Err(e),
        Err(LocalVoteRejected::AlreadyVoted) => Err(ApiError::Conflict("ALREADY_VOTED")),
        Err(LocalVoteRejected::InvalidPoll) => Err(ApiError::BadRequest("INVALID_POLL".to_owned())),
    }
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
