use super::*;

// Undo(Follow) アクティビティを処理してフォロー解除する
pub(super) async fn handle_undo(
    activity: serde_json::Value,
    inbox: &InboxContext,
) -> Result<(), String> {
    let obj = &activity["object"];

    // Undo(Like) / Undo(EmojiReact): reactions から対象を削除する (#22)
    if matches!(obj["type"].as_str(), Some("Like") | Some("EmojiReact")) {
        if let Some(activity_id) = obj["id"].as_str() {
            let deleted = inbox
                .reaction_repo
                .delete_by_activity_id(activity_id)
                .await
                .map_err(|e| format!("reactions DELETE エラー: {}", e))?;
            if let Some((post_id, actor_id)) = deleted {
                tracing::info!(
                    "[Undo/Reaction] {} を取り消し（post_id={}）",
                    activity_id,
                    post_id
                );
                if let Ok(Some(post)) = inbox.post_repo.find_by_id(post_id).await {
                    broadcast_reaction_update(
                        &inbox.stream_hub,
                        inbox.follow_repo.as_ref(),
                        inbox.reaction_repo.as_ref(),
                        post_id,
                        post.actor_id,
                        actor_id,
                        None,
                    )
                    .await;
                }
            }
        }
        return Ok(());
    }

    // Undo(Block): handle_block で記録した相手発ブロック（blocker=相手, blocked=ローカル）を
    // 削除する（自動再フォローはしない）。
    if obj["type"].as_str() == Some("Block") {
        let blocker_uri = activity["actor"].as_str().unwrap_or("");
        let target_uri = obj["object"].as_str().unwrap_or("");
        // ホスト名まで検証する（handle_block と同じ理由）。
        let local_username =
            crate::ap::extract_local_username(target_uri, &inbox.local_domain).unwrap_or("");

        if let (Some(blocker), Some(target)) = (
            inbox
                .actor_repo
                .find_by_ap_uri(blocker_uri)
                .await
                .ok()
                .flatten(),
            inbox
                .actor_repo
                .find_by_username_domain(local_username, &inbox.local_domain)
                .await
                .ok()
                .flatten(),
        ) {
            if target.actor_type == "local" {
                if let Err(e) = inbox
                    .block_repo
                    .delete_by_actors(blocker.id, target.id)
                    .await
                {
                    tracing::error!("[Undo/Block] blocks DELETE エラー: {}", e);
                }
            }
        }

        tracing::info!(
            "[Undo/Block] {} からのブロック解除を受信しました",
            blocker_uri
        );
        return Ok(());
    }

    // Undo(Announce): posts から対象のリポストを論理削除する
    if obj["type"].as_str() == Some("Announce") {
        if let Some(announce_id) = obj["id"].as_str() {
            let deleted = inbox
                .post_repo
                .soft_delete_by_ap_object_id(announce_id)
                .await
                .map_err(|e| format!("posts (Announce) UPDATE エラー: {}", e))?;
            tracing::info!(
                "[Undo/Announce] {} を取り消し（{} 行）",
                announce_id,
                deleted
            );
        }
        return Ok(());
    }

    if obj["type"].as_str() != Some("Follow") {
        return Ok(());
    }

    let follower_uri = activity["actor"]
        .as_str()
        .ok_or("Undo: actor フィールドがありません")?;
    let target_uri = obj["object"]
        .as_str()
        .ok_or("Undo/Follow: object.object フィールドがありません")?;

    // ホスト名まで検証する（handle_follow と同じ理由）。
    let local_username = crate::ap::extract_local_username(target_uri, &inbox.local_domain)
        .ok_or("Undo/Follow: object.object URI が自ドメインのアクターを指していません")?;

    let follower = match inbox
        .actor_repo
        .find_by_ap_uri(follower_uri)
        .await
        .map_err(|e| format!("フォロワーアクター検索エラー: {}", e))?
    {
        Some(a) => a,
        None => return Ok(()), // 既にいない場合は何もしない
    };

    let target = match inbox
        .actor_repo
        .find_by_username_domain(local_username, &inbox.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
    {
        Some(a) if a.actor_type == "local" => a,
        _ => return Ok(()),
    };

    inbox
        .follow_repo
        .delete_by_actors(follower.id, target.id)
        .await
        .map_err(|e| format!("follows DELETE エラー: {}", e))?;

    tracing::info!("[Undo/Follow] {} のフォロー解除完了", follower_uri);
    Ok(())
}
