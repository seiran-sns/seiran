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

    // 投票の記録と票数加算を同一トランザクションで行う（加算自体は`increment_poll_votes`が
    // 単一UPDATE内で行うため、ローカル投票・他のリモート投票と同時でも加算は失われない）。
    let mut tx = inbox
        .db_pool
        .begin()
        .await
        .map_err(|e| format!("PollVote: トランザクション開始失敗: {}", e))?;
    let inserted = sqlx::query(
        "INSERT INTO poll_votes (post_id, actor_id, option_index, ap_activity_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING",
    )
    .bind(post_id)
    .bind(remote.actor_id)
    .bind(index as i32)
    .bind(activity_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("PollVote: 保存失敗: {}", e))?;
    let updated = if inserted.rows_affected() > 0 {
        crate::repository::poll::increment_poll_votes(&mut tx, post_id, &[index as i32])
            .await
            .map_err(|e| format!("PollVote: 集計更新失敗: {}", e))?
    } else {
        None
    };
    tx.commit()
        .await
        .map_err(|e| format!("PollVote: コミット失敗: {}", e))?;
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
