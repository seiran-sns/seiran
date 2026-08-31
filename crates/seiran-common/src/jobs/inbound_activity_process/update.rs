use super::note_input::normalize_ap_poll;
use super::*;

/// `Update`アクティビティのうち、アンケート（`object.type == "Question"`）のみを受理する。
/// 本文再編集のUpdate（`object.type == "Note"`等）は別件のため今回は非対応、黙って無視する。
/// Update(Question)を受理できたNoteは`posts.poll_update_received`をtrueにし、以後
/// `Job::PollFetch`（生存監視フォールバック）の対象から外す（送信元がpush型実装と判明したため）。
pub(super) async fn handle_update(
    activity: serde_json::Value,
    inbox: &InboxContext,
) -> Result<(), String> {
    let object = &activity["object"];
    if object["type"].as_str() != Some("Question") {
        return Ok(());
    }
    let Some(question_id) = object["id"].as_str() else {
        return Ok(());
    };
    let Some(poll) = normalize_ap_poll(object) else {
        return Ok(());
    };

    let Some((post_id, post_author_id)) = inbox
        .post_repo
        .find_id_and_actor_by_ap_object_id(question_id)
        .await
        .map_err(|e| format!("Update: Question検索失敗: {}", e))?
    else {
        return Ok(());
    };

    // なりすまし対策: Update の送信元（HTTP Signature 検証済みの actor）が投稿者本人か確認する
    // （`delete.rs::handle_delete`と同じパターン）。
    let actor_uri = activity["actor"].as_str().unwrap_or("");
    let sender = inbox
        .actor_repo
        .find_by_ap_uri(actor_uri)
        .await
        .map_err(|e| format!("送信元アクター検索エラー: {}", e))?;
    if sender.map(|a| a.id) != Some(post_author_id) {
        tracing::warn!(
            "[Update] {} の送信元アクター({})が投稿の所有者と一致しないため無視します",
            question_id,
            actor_uri
        );
        return Ok(());
    }

    sqlx::query(
        "UPDATE posts SET poll = $2, poll_update_received = true, poll_fetched_at = now()
         WHERE id = $1",
    )
    .bind(post_id)
    .bind(&poll)
    .execute(&inbox.db_pool)
    .await
    .map_err(|e| format!("Update: poll更新失敗: {}", e))?;

    broadcast_poll_update(
        &inbox.stream_hub,
        inbox.follow_repo.as_ref(),
        post_id,
        post_author_id,
        &poll,
    )
    .await;

    Ok(())
}
