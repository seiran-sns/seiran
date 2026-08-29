use super::*;
use queries::find_repost_for_undo;


/// DELETE /api/notes/:note_id/repost
/// 自分がしたリポストを取り消す（論理削除 + AP Undo/Announce 配送）。
pub async fn delete_repost(
    Path(note_id_str): Path<String>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = user.actor_id;

    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    // 削除前にリポスト行の id・ap_object_id・atp_repost_rkey と元ポストの ap_object_id を取得する
    let undo_info = match find_repost_for_undo(&state, actor_id, note_id).await {
        Ok(info) => info,
        Err(resp) => return resp,
    };

    // 論理削除
    if let Err(e) = state.posts.soft_delete_by_id(undo_info.repost_id).await {
        return ApiError::Internal(format!("UPDATE 失敗: {}", e)).into_response();
    }

    tracing::info!(
        "[delete_repost] actor_id={} が note_id={} のリポスト（post_id={}）を取り消し",
        actor_id,
        note_id,
        undo_info.repost_id
    );

    // AP Undo(Announce) 配送 — 元ポストに ap_object_id がある場合のみ。
    // 元ポストが Bsky ネイティブ（ap_object_id 無し・at_uri 有り）の場合、Fedi へは
    // Announce ではなく PostToFollowers の Create(Note) フォールバックを送っているため、
    // Undo(Announce) ではなく Delete(Note) でその Note を撤回する。
    if let Some(orig_ap_object_id) = undo_info.orig_ap_id {
        state
            .enqueue_ap_delivery(
                actor_id,
                ApDeliveryKind::UndoAnnounce {
                    announce_post_id: undo_info.repost_id,
                    original_ap_object_id: orig_ap_object_id,
                },
            )
            .await;
    } else if undo_info.orig_at_uri.is_some() {
        state
            .enqueue_ap_delivery(
                actor_id,
                ApDeliveryKind::DeleteNote {
                    post_id: undo_info.repost_id,
                },
            )
            .await;
    }

    // ATP repost delete commit — atp_repost_rkey が保存されている場合のみ
    if let Some(rkey) = undo_info.atp_repost_rkey {
        let atp = Arc::clone(&state.atp_service);
        let now = chrono::Utc::now();
        tokio::spawn(async move {
            if let Err(e) = atp.delete_atp_repost(actor_id, &rkey, now).await {
                tracing::error!("[delete_repost] ATP repost delete 失敗: {}", e);
            }
        });
    } else if let Some(rkey) = undo_info.at_rkey {
        // Fedi リモートポストのリポスト時に作った Bsky フォールバックテキスト投稿を retract する。
        let atp = Arc::clone(&state.atp_service);
        let now = chrono::Utc::now();
        tokio::spawn(async move {
            if let Err(e) = atp.delete_atp_post(actor_id, &rkey, now).await {
                tracing::error!("[delete_repost] Bsky フォールバック投稿 delete 失敗: {}", e);
            }
        });
    }

    Json(serde_json::json!({
        "ok": true,
        "repostId": undo_info.repost_ap_id.unwrap_or_default()
    }))
    .into_response()
}

/// DELETE /api/notes/:id
/// 自分の投稿を削除する。論理削除（`deleted_at`）に加え、実際に配送済みだった先
/// （Fedi/Bsky）へ Delete/取り消しを配送する。リポスト・引用・返信・リアクション等の
/// 関連行はカスケード削除しない（読み取り側が `deleted_at IS NULL` を一貫して見る設計）。
pub async fn delete_note(
    Path(note_id_str): Path<String>,
    me: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let info = match state.posts.find_delete_info(note_id).await {
        Ok(Some(info)) => info,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    };
    if info.actor_id != me.actor_id {
        return ApiError::Forbidden("NOT_YOUR_POST").into_response();
    }

    if let Err(e) = state.posts.soft_delete_by_id(note_id).await {
        return ApiError::Internal(format!("UPDATE 失敗: {}", e)).into_response();
    }

    tracing::info!(
        "[delete_note] actor_id={} が note_id={} を削除",
        me.actor_id,
        note_id
    );

    // AP Delete(Note) 配送 — 実際に Fedi へ Create(Note) 済みの場合のみ。direct（DM）は
    // フォロワー配送ロジックしか持たないため対象外（本来の宛先には届かない）。
    if info.deliver_fedi && info.visibility != "direct" {
        state
            .enqueue_ap_delivery(me.actor_id, ApDeliveryKind::DeleteNote { post_id: note_id })
            .await;
    }

    // ATP 投稿 delete commit — Bsky へコミット済みで rkey が保存されている場合のみ
    if let Some(rkey) = info.at_rkey {
        let atp = Arc::clone(&state.atp_service);
        let now = chrono::Utc::now();
        tokio::spawn(async move {
            if let Err(e) = atp.delete_atp_post(me.actor_id, &rkey, now).await {
                tracing::error!("[delete_note] ATP post delete 失敗: {}", e);
            }
        });
    }

    Json(serde_json::json!({ "ok": true })).into_response()
}
