use super::*;
use profile_material::sync_bsky_pinned_post;

/// POST /api/notes/:id/pin
/// 自分の投稿をピン留めする（#61）。5件を超えると最古のピン留めが自動的に外れる。
/// Fedi 向けは featured collection（都度動的生成、`seiran-federation-inbox`）で、
/// Bsky 向けは最新1件のみ `app.bsky.actor.profile` の `pinnedPost` として反映する。
pub async fn pin_note(
    Path(note_id_str): Path<String>,
    me: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let post = match state
        .posts
        .find_by_id_for_viewer(note_id, Some(me.actor_id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    };
    if post.actor_id != me.actor_id {
        return ApiError::Forbidden("NOT_YOUR_POST").into_response();
    }

    if let Err(e) = state.pinned_posts.pin(me.actor_id, note_id).await {
        return ApiError::Internal(format!("pinned_posts INSERT 失敗: {}", e)).into_response();
    }

    sync_bsky_pinned_post(&state, me.actor_id).await;

    respond_with_pinned_ids(&state, me.actor_id).await
}

/// DELETE /api/notes/:id/pin
/// 自分の投稿のピン留めを解除する（#61）。
pub async fn unpin_note(
    Path(note_id_str): Path<String>,
    me: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    match state.pinned_posts.unpin(me.actor_id, note_id).await {
        Ok(true) => {}
        Ok(false) => return ApiError::NotFound("PIN_NOT_FOUND").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("pinned_posts DELETE 失敗: {}", e)).into_response()
        }
    }

    sync_bsky_pinned_post(&state, me.actor_id).await;

    respond_with_pinned_ids(&state, me.actor_id).await
}

async fn respond_with_pinned_ids(state: &AppState, actor_id: i64) -> Response {
    match state.pinned_posts.list_by_actor(actor_id).await {
        Ok(ids) => Json(serde_json::json!({
            "ok": true,
            "pinnedPostIds": ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => ApiError::Internal(format!("pinned_posts SELECT 失敗: {}", e)).into_response(),
    }
}
