use super::*;
use queries::fetch_reposted_ids;

pub async fn home_timeline(
    Query(q): Query<TimelineQuery>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = user.actor_id;

    let limit = q.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let mut rows = match state
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
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let mut lc_map = fetch_link_cards_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, Some(actor_id)).await;
    let reposted_set = fetch_reposted_ids(&state.db, actor_id, &ids).await;
    let mut notes: Vec<NoteResponse> = rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(
                p,
                att_map.remove(&id).unwrap_or_default(),
                lc_map.remove(&id).unwrap_or_default(),
            );
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            nr.reposted_by_me = Some(reposted_set.contains(&id));
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut notes, Some(actor_id)).await;
    embed_quotes(&state.db, &mut notes, Some(actor_id)).await;
    attach_poll_votes(&state.db, &mut notes, Some(actor_id)).await;
    attach_reply_quote_gates(&state, &mut notes, Some(actor_id)).await;
    attach_remote_instance_info(&state, &mut notes).await;
    attach_relationship_flags(&state, &mut notes, Some(actor_id)).await;
    enqueue_stale_poll_fetches(&state, &notes).await;
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

    let mut rows = match state
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
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let mut lc_map = fetch_link_cards_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, my_actor_id).await;
    let reposted_set = if let Some(actor_id) = my_actor_id {
        fetch_reposted_ids(&state.db, actor_id, &ids).await
    } else {
        Default::default()
    };
    let mut notes: Vec<NoteResponse> = rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(
                p,
                att_map.remove(&id).unwrap_or_default(),
                lc_map.remove(&id).unwrap_or_default(),
            );
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            if my_actor_id.is_some() {
                nr.reposted_by_me = Some(reposted_set.contains(&id));
            }
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut notes, my_actor_id).await;
    embed_quotes(&state.db, &mut notes, my_actor_id).await;
    attach_poll_votes(&state.db, &mut notes, my_actor_id).await;
    attach_reply_quote_gates(&state, &mut notes, my_actor_id).await;
    attach_remote_instance_info(&state, &mut notes).await;
    attach_relationship_flags(&state, &mut notes, my_actor_id).await;
    enqueue_stale_poll_fetches(&state, &notes).await;
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

    let mut rows = match state
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
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let mut lc_map = fetch_link_cards_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, Some(actor_id)).await;
    let reposted_set = fetch_reposted_ids(&state.db, actor_id, &ids).await;
    let mut notes: Vec<NoteResponse> = rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(
                p,
                att_map.remove(&id).unwrap_or_default(),
                lc_map.remove(&id).unwrap_or_default(),
            );
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            nr.reposted_by_me = Some(reposted_set.contains(&id));
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut notes, Some(actor_id)).await;
    embed_quotes(&state.db, &mut notes, Some(actor_id)).await;
    attach_poll_votes(&state.db, &mut notes, Some(actor_id)).await;
    attach_reply_quote_gates(&state, &mut notes, Some(actor_id)).await;
    attach_remote_instance_info(&state, &mut notes).await;
    attach_relationship_flags(&state, &mut notes, Some(actor_id)).await;
    enqueue_stale_poll_fetches(&state, &notes).await;
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

    let mut rows = match state
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
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let mut lc_map = fetch_link_cards_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, my_actor_id).await;
    let reposted_set = if let Some(actor_id) = my_actor_id {
        fetch_reposted_ids(&state.db, actor_id, &ids).await
    } else {
        Default::default()
    };
    let mut notes: Vec<NoteResponse> = rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(
                p,
                att_map.remove(&id).unwrap_or_default(),
                lc_map.remove(&id).unwrap_or_default(),
            );
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            if my_actor_id.is_some() {
                nr.reposted_by_me = Some(reposted_set.contains(&id));
            }
            nr
        })
        .collect();
    embed_renotes(&state.db, &mut notes, my_actor_id).await;
    embed_quotes(&state.db, &mut notes, my_actor_id).await;
    attach_poll_votes(&state.db, &mut notes, my_actor_id).await;
    attach_reply_quote_gates(&state, &mut notes, my_actor_id).await;
    attach_remote_instance_info(&state, &mut notes).await;
    attach_relationship_flags(&state, &mut notes, my_actor_id).await;
    enqueue_stale_poll_fetches(&state, &notes).await;
    Json(notes).into_response()
}
