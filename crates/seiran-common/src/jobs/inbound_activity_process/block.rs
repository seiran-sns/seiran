use super::*;

/// Block アクティビティを処理する。相手発のブロックを `blocks` テーブルへ記録する
/// （`blocker_actor_id=相手, blocked_actor_id=ローカル`。方向性を持つ関係として素直に
/// 記録するだけであり視点混在にはならない）。これにより `actor_is_hidden_for_viewer`
/// による相互非表示・書き込みガードが自動的に有効になる（`docs/protocols.md` 10節）。
/// あわせて、ブロックされた側がブロックした側をフォローしていた関係があれば解消する
/// （Mastodon 等の実挙動に合わせる）。通知は生成しない（Fedi慣習：ブロックは本人に知らせない）。
pub(super) async fn handle_block(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let blocker_uri = activity["actor"]
        .as_str()
        .ok_or("Block: actor フィールドがありません")?;
    let target_uri = activity["object"]
        .as_str()
        .ok_or("Block: object フィールドがありません")?;

    // ホスト名まで確認する（handle_follow と同じ理由。リモートの同名ユーザーの
    // Block をローカルの同名ユーザーへの Block と誤認しないため）。
    let local_username = crate::ap::extract_local_username(target_uri, &inbox.local_domain)
        .ok_or("Block: object URI が自ドメインのアクターを指していません")?;

    let local_actor = inbox
        .actor_repo
        .find_including_withdrawn_by_username_domain(local_username, &inbox.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("ローカルアクター '{}' が存在しません", local_username))?;
    if local_actor.actor_type != "local" {
        return Err(format!(
            "'{}' はローカルアクターではありません",
            local_username
        ));
    }

    let remote = upsert_remote_fedi_actor(inbox, ap_client, blocker_uri).await?;

    // 相手発のブロックを記録する（Fedi側にはrkeyの概念が無いため atp_rkey は None）。
    inbox
        .block_repo
        .insert(remote.actor_id, local_actor.id, None)
        .await
        .map_err(|e| format!("blocks INSERT エラー: {}", e))?;

    // こちら（ブロックされた側）が相手をフォローしていた関係を解消する。
    inbox
        .follow_repo
        .delete_by_actors(local_actor.id, remote.actor_id)
        .await
        .map_err(|e| format!("follows DELETE エラー: {}", e))?;

    tracing::info!(
        "[Block] {} から '{}' へのブロックを受信・記録し、フォロー関係を解消しました",
        blocker_uri,
        local_username
    );
    Ok(())
}
