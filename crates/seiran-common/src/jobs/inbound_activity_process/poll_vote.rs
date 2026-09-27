use super::*;

pub(super) async fn handle_poll_vote(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("PollVote: actor がありません")?;
    let object = &activity["object"];
    let question_id = object["inReplyTo"]
        .as_str()
        .ok_or("PollVote: inReplyTo がありません")?;
    let option_name = object["name"]
        .as_str()
        .ok_or("PollVote: name がありません")?;
    let activity_id = activity["id"].as_str().or_else(|| object["id"].as_str());

    let Some((post_id, post_author_id)) = inbox
        .post_repo
        .find_id_and_actor_by_ap_object_id(question_id)
        .await
        .map_err(|e| format!("PollVote: Question検索失敗: {}", e))?
    else {
        return Ok(());
    };
    let remote = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let poll = crate::repository::poll::poll_of(&inbox.db_pool, post_id)
        .await
        .map_err(|e| format!("PollVote: poll取得失敗: {}", e))?;
    let Some(poll) = poll else { return Ok(()) };
    let Some(index) = poll["options"].as_array().and_then(|options| {
        options
            .iter()
            .position(|o| o["name"].as_str() == Some(option_name))
    }) else {
        return Ok(());
    };

    let updated = crate::repository::poll::record_remote_vote(
        &inbox.db_pool,
        post_id,
        remote.actor_id,
        index as i32,
        activity_id,
    )
    .await
    .map_err(|e| format!("PollVote: 保存失敗: {}", e))?;
    if let Some(updated) = updated {
        // タイムライン/ノート詳細のアンケート結果をリアルタイム更新する
        // （`broadcast_reaction_update` と同じ考え方）。
        broadcast_poll_update(
            &inbox.stream_hub,
            inbox.follow_repo.as_ref(),
            post_id,
            post_author_id,
            &updated,
        )
        .await;
    }
    if post_author_id != remote.actor_id {
        inbox.stream_hub.publish_event(
            HashSet::from([post_author_id]), "pollVote",
            serde_json::json!({"postId": post_id.to_string(), "actorId": remote.actor_id.to_string()}),
        );
    }
    Ok(())
}
