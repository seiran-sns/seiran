use super::*;
use super::infra::*;
use super::activity::*;


/// リアクション配送先を解決する。
///
/// 配送先は (1) 対象ポストの著者（Fedi リモートの場合のみ）と (2) `reactor_actor_id`
/// の Fedi フォロワー全員、の inbox の和集合（重複排除）。対象ポストが AP 上の実体
/// （`ap_object_id`）を持たない場合（Bsky 由来など）は `None` を返し、配送不要とする。
async fn resolve_reaction_targets(
    db: &PgPool,
    post_id: i64,
    reactor_actor_id: i64,
) -> Result<Option<(String, Vec<String>)>, ApError> {
    let post_row = sqlx::query(
        "SELECT p.ap_object_id, a.actor_type::text AS actor_type, a.ap_inbox_url
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 LIMIT 1",
    )
    .bind(post_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("対象ポスト取得エラー: {}", e)))?;

    let post_row = match post_row {
        Some(r) => r,
        None => return Ok(None),
    };

    let object_ap_id: Option<String> = post_row.try_get("ap_object_id").unwrap_or(None);
    let object_ap_id = match object_ap_id {
        Some(id) => id,
        None => return Ok(None),
    };
    let author_actor_type: String = post_row.try_get("actor_type").unwrap_or_default();
    let author_inbox: Option<String> = post_row.try_get("ap_inbox_url").unwrap_or(None);

    let mut inboxes: std::collections::HashSet<String> = std::collections::HashSet::new();
    if author_actor_type == "fedi" {
        if let Some(inbox) = author_inbox {
            inboxes.insert(inbox);
        }
    }

    inboxes.extend(fetch_fedi_follower_inboxes(db, reactor_actor_id).await?);

    Ok(Some((object_ap_id, inboxes.into_iter().collect())))
}

/// ローカルアクターの絵文字リアクション（Like/EmojiReact）を、対象ポストの著者
/// （Fedi リモートの場合のみ）と reactor 本人の Fedi フォロワー全員の inbox へ配送する。
///
/// `activity_id` は呼び出し元があらかじめ発行し `reactions.ap_activity_id` に保存した値と
/// 同一のものを渡すこと（後の Undo で参照するため）。
#[allow(clippy::too_many_arguments)]
pub async fn deliver_ap_reaction(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    activity_id: &str,
    content: &str,
    emoji_url: Option<&str>,
) -> Result<(), ApError> {
    let (object_ap_id, inboxes) = match resolve_reaction_targets(db, post_id, actor_id).await? {
        Some(v) => v,
        None => return Ok(()),
    };

    let username = fetch_username(db, actor_id).await?;
    let addr = local_actor_address(local_domain, &username);
    let activity_type = reaction_activity_type(content);

    let mut activity = build_reaction_object(
        activity_type,
        activity_id,
        &addr.actor_uri,
        &object_ap_id,
        content,
        emoji_url,
        local_domain,
    );
    activity["@context"] =
        serde_json::Value::String("https://www.w3.org/ns/activitystreams".to_string());
    activity["published"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
    activity["to"] = serde_json::json!(["https://www.w3.org/ns/activitystreams#Public"]);
    activity["cc"] = serde_json::json!([addr.followers_uri]);

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "{} post_id={} actor_id={}",
            activity_type, post_id, actor_id
        ),
    )
    .await
}

/// リモートQuestionへの回答を、Mastodon互換の
/// `Create { object: Note { name, inReplyTo } }` として投稿者inboxへ送る。
pub async fn deliver_ap_poll_vote(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    option_names: &[String],
) -> Result<(), ApError> {
    let row = sqlx::query(
        "SELECT p.ap_object_id, a.ap_inbox_url, a.ap_uri
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 AND p.deleted_at IS NULL",
    )
    .bind(post_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("アンケート配送先取得エラー: {}", e)))?;
    let Some(row) = row else { return Ok(()) };
    let Some(question_id): Option<String> = row.try_get("ap_object_id").unwrap_or(None) else {
        return Ok(());
    };
    let Some(inbox): Option<String> = row.try_get("ap_inbox_url").unwrap_or(None) else {
        return Ok(());
    };
    let Some(author_uri): Option<String> = row.try_get("ap_uri").unwrap_or(None) else {
        return Ok(());
    };

    let username = fetch_username(db, actor_id).await?;
    let addr = local_actor_address(local_domain, &username);
    for (index, name) in option_names.iter().enumerate() {
        let activity_id = format!(
            "https://{}/activities/poll-vote-{}-{}-{}",
            local_domain, post_id, actor_id, index
        );
        let note_id = format!("{}/note", activity_id);
        let activity = serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": activity_id,
            "type": "Create",
            "actor": addr.actor_uri,
            "to": [author_uri],
            "object": {
                "id": note_id,
                "type": "Note",
                "attributedTo": addr.actor_uri,
                "name": name,
                "inReplyTo": question_id,
                "to": [author_uri]
            }
        });
        fan_out_activity(
            ap_client,
            std::slice::from_ref(&inbox),
            &activity,
            &addr.key_id,
            ap_private_key_pem,
            &format!("PollVote post_id={} actor_id={}", post_id, actor_id),
        )
        .await?;
    }
    Ok(())
}

/// ローカルアクターの絵文字リアクション取消（Undo(Like)/Undo(EmojiReact)）を、
/// `deliver_ap_reaction` と同じ宛先集合（対象ポスト著者 + reactor 本人の Fedi フォロワー）へ配送する。
///
/// `prev_activity_id` / `content` は取り消し対象の元リアクションのもの
/// （`reactions.ap_activity_id` に保存されていた値とその時点の `content`）を渡すこと。
#[allow(clippy::too_many_arguments)]
pub async fn deliver_ap_undo_reaction(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    prev_activity_id: &str,
    content: &str,
    emoji_url: Option<&str>,
) -> Result<(), ApError> {
    let (object_ap_id, inboxes) = match resolve_reaction_targets(db, post_id, actor_id).await? {
        Some(v) => v,
        None => return Ok(()),
    };

    let username = fetch_username(db, actor_id).await?;
    let addr = local_actor_address(local_domain, &username);
    let activity_type = reaction_activity_type(content);
    let inner = build_reaction_object(
        activity_type,
        prev_activity_id,
        &addr.actor_uri,
        &object_ap_id,
        content,
        emoji_url,
        local_domain,
    );

    let undo_id = format!(
        "https://{}/activities/undo-reactions/{}-{}-{}",
        local_domain,
        post_id,
        actor_id,
        chrono::Utc::now().timestamp_millis()
    );
    let activity =
        build_undo_reaction_activity(&addr, &undo_id, &chrono::Utc::now().to_rfc3339(), inner);

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Undo({}) post_id={} actor_id={}",
            activity_type, post_id, actor_id
        ),
    )
    .await
}
