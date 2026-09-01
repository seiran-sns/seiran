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
    let poll: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT poll FROM posts WHERE id = $1")
            .bind(post_id)
            .fetch_optional(&inbox.db_pool)
            .await
            .map_err(|e| format!("PollVote: poll取得失敗: {}", e))?
            .flatten();
    let Some(poll) = poll else { return Ok(()) };
    let Some(index) = poll["options"].as_array().and_then(|options| {
        options
            .iter()
            .position(|o| o["name"].as_str() == Some(option_name))
    }) else {
        return Ok(());
    };

    let inserted = sqlx::query(
        "INSERT INTO poll_votes (post_id, actor_id, option_index, ap_activity_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING",
    )
    .bind(post_id)
    .bind(remote.actor_id)
    .bind(index as i32)
    .bind(activity_id)
    .execute(&inbox.db_pool)
    .await
    .map_err(|e| format!("PollVote: 保存失敗: {}", e))?;
    if inserted.rows_affected() > 0 {
        let mut updated = poll;
        if let Some(option) = updated["options"]
            .as_array_mut()
            .and_then(|options| options.get_mut(index))
        {
            option["votes"] = serde_json::json!(option["votes"].as_i64().unwrap_or(0) + 1);
        }
        sqlx::query("UPDATE posts SET poll = $2 WHERE id = $1")
            .bind(post_id)
            .bind(&updated)
            .execute(&inbox.db_pool)
            .await
            .map_err(|e| format!("PollVote: 集計更新失敗: {}", e))?;
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
