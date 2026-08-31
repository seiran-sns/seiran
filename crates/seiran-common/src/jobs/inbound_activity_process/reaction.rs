use super::*;
use super::emoji::{extract_emoji_tag_url, record_remote_emojis};


/// いいね（Like）・絵文字リアクション（EmojiReact）を受信し reactions テーブルへ保存する (#22)。
///
/// Misskey は絵文字リアクション（Unicode 絵文字・カスタム絵文字とも）でも AP の `type` を
/// `"Like"` 固定で送り、実際の内容は `content`/`_misskey_reaction` フィールドに載せる
/// （`EmojiReact` 型は使わない）。そのため種別判定に wire type を使わず、`content` /
/// `_misskey_reaction` の値の有無で決める（どちらも無い場合のみ、Mastodon 等の素の
/// お気に入りとみなし ❤️ を割り当てる）。
pub(super) async fn handle_reaction(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("Reaction: actor フィールドがありません")?;
    // object は対象ノートの URI（文字列 or {id}）
    let object_uri = activity["object"]
        .as_str()
        .or_else(|| activity["object"]["id"].as_str())
        .ok_or("Reaction: object フィールドがありません")?;
    let activity_id = activity["id"].as_str();

    let content: String = activity["content"]
        .as_str()
        .or_else(|| activity["_misskey_reaction"].as_str())
        .unwrap_or("❤️")
        .to_string();
    let reaction_type = if content == "❤️" { "like" } else { "emoji" };
    // content が `:shortcode:` 形式（カスタム絵文字）の場合、tag 配列から画像 URL を解決する。
    // Unicode 絵文字や素の Like（❤️ 固定）では通常 tag に一致が無いため自然に None になる。
    let emoji_url = extract_emoji_tag_url(&activity, &content);

    // 対象ローカルポストを ap_object_id で検索（未知のポストなら無視）
    let (post_id, post_author_id) = match inbox
        .post_repo
        .find_id_and_actor_by_ap_object_id(object_uri)
        .await
        .map_err(|e| format!("対象ポスト検索エラー: {}", e))?
    {
        Some(pair) => pair,
        None => return Ok(()), // 未知ポストへのリアクションは無視
    };

    // リアクションを打ったアクターを解決・upsert
    let remote = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let actor_id = remote.actor_id;

    // カスタム絵文字リアクションなら remote_emojis にも記録する（#73）。
    if let Some(url) = emoji_url.as_deref() {
        let tag = serde_json::json!({
            "type": "Emoji",
            "name": content,
            "icon": { "url": url },
        });
        record_remote_emojis(inbox, &remote.domain, &[tag]).await;
    }

    // reactions へ INSERT（同一ユーザー・同一内容の重複、activity_id 重複はスキップ）
    let new_reaction_id = generate_snowflake_id(chrono::Utc::now());
    inbox
        .reaction_repo
        .insert(
            new_reaction_id,
            post_id,
            actor_id,
            reaction_type,
            &content,
            activity_id,
            None,
            emoji_url.as_deref(),
        )
        .await
        .map_err(|e| format!("reactions INSERT エラー: {}", e))?;

    tracing::info!("[Reaction] post {} に {} を記録", post_id, content);

    // 通知ベル用（#37）: リアクションされたポストの著者へ
    inbox.stream_hub.publish_event(
        HashSet::from([post_author_id]),
        "reaction",
        serde_json::json!({
            "postId": post_id.to_string(),
            "emoji": content,
            "emojiUrl": emoji_url,
            "actor": { "username": remote.username, "domain": remote.domain, "displayName": remote.display_name },
        }),
    );
    let notif_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = inbox
        .notification_repo
        .insert(
            notif_id,
            post_author_id,
            NotificationKind::Reaction,
            Some(actor_id),
            Some(post_id),
            Some(&content),
            emoji_url.as_deref(),
            activity_id,
            None,
            None,
        )
        .await
    {
        tracing::error!("[Reaction] notifications INSERT 失敗: {}", e);
    }

    // タイムライン/ノート詳細のリアクション表示をリアルタイム更新する（Misskey 互換の
    // ストリーミング挙動に合わせる）。
    broadcast_reaction_update(
        &inbox.stream_hub,
        inbox.follow_repo.as_ref(),
        inbox.reaction_repo.as_ref(),
        post_id,
        post_author_id,
        actor_id,
        Some(&content),
    )
    .await;

    Ok(())
}
