use super::activity::*;
use super::infra::*;
use super::*;

/// リアクション配送先。
struct ReactionTargets {
    object_ap_id: String,
    inboxes: Vec<String>,
    /// activityの`to`にそのまま入れる値。DM宛は宛先のap_uriのみ、通常投稿はPublicのURI。
    to: Vec<String>,
    /// `true`ならDM（`visibility='direct'`）宛。`cc`（フォロワー宛）を付けない。
    is_dm: bool,
}

/// リアクション配送先を解決する。
///
/// 対象ポストが`visibility='direct'`（DM）の場合、そのメッセージの「会話参加者」
/// （投稿者本人 + そのメッセージの宛先`post_recipients`、reactor自身は除く）のうち
/// Fediアクターのinboxのみへ配送し、`to`もその参加者のap_uriのみに絞る（DMの存在自体が
/// 第三者へ漏れることを防ぐため、通常投稿と同じPublic+フォロワー全体配送は絶対に使わない）。
/// 「宛先(`post_recipients`)」だけを見ると、自分宛に届いたメッセージへリアクションした際に
/// 本来の配送先である投稿者（相手）が漏れてしまう（宛先には自分自身しか含まれないため）。
///
/// それ以外（通常投稿）の配送先は `reactor_actor_id` の Fedi フォロワー全員に加え、対象
/// ポストを巡る会話の参加者（対象ポストの著者とそのフォロワー、対象ポストへの子ポスト＝
/// リポスト/返信/引用の投稿者とそのフォロワー、対象ポストに付いている絵文字リアクションの
/// reactor）の inbox の和集合（重複排除、#235。詳細は `resolve_conversation_broadcast_inboxes`
/// 参照）。
/// 対象ポストが AP 上の実体（`ap_object_id`）を持たない場合（Bsky 由来など）は `None` を
/// 返し、配送不要とする。
async fn resolve_reaction_targets(
    db: &PgPool,
    post_id: i64,
    reactor_actor_id: i64,
) -> Result<Option<ReactionTargets>, ApError> {
    let row = sqlx::query(
        "SELECT ap_object_id, visibility::text AS visibility FROM posts WHERE id = $1 LIMIT 1",
    )
    .bind(post_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("対象ポスト取得エラー: {}", e)))?;
    let Some(row) = row else { return Ok(None) };
    let object_ap_id: Option<String> = row.try_get("ap_object_id").unwrap_or(None);
    let Some(object_ap_id) = object_ap_id else {
        return Ok(None);
    };
    let visibility: String = row.try_get("visibility").unwrap_or_default();

    if visibility == "direct" {
        let participant_rows = sqlx::query(
            "SELECT DISTINCT a.ap_uri, a.ap_inbox_url
             FROM (
                 SELECT p.actor_id AS aid FROM posts p WHERE p.id = $1
                 UNION
                 SELECT pr.actor_id AS aid FROM post_recipients pr WHERE pr.post_id = $1
             ) participants
             JOIN actors a ON a.id = participants.aid
             WHERE participants.aid != $2
               AND a.actor_type IN ('fedi', 'remote_seiran') AND a.ap_uri IS NOT NULL AND a.ap_inbox_url IS NOT NULL",
        )
        .bind(post_id)
        .bind(reactor_actor_id)
        .fetch_all(db)
        .await
        .map_err(|e| ApError::Other(format!("DM会話参加者取得エラー: {}", e)))?;
        if participant_rows.is_empty() {
            return Ok(None);
        }
        let to: Vec<String> = participant_rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>("ap_uri").ok())
            .collect();
        let inboxes: Vec<String> = participant_rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>("ap_inbox_url").ok())
            .collect();
        return Ok(Some(ReactionTargets {
            object_ap_id,
            inboxes,
            to,
            is_dm: true,
        }));
    }

    let mut inboxes = resolve_conversation_broadcast_inboxes(db, post_id).await?;
    inboxes.extend(fetch_fedi_follower_inboxes(db, reactor_actor_id).await?);

    Ok(Some(ReactionTargets {
        object_ap_id,
        inboxes: inboxes.into_iter().collect(),
        to: vec!["https://www.w3.org/ns/activitystreams#Public".to_string()],
        is_dm: false,
    }))
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
    let targets = match resolve_reaction_targets(db, post_id, actor_id).await? {
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
        &targets.object_ap_id,
        content,
        emoji_url,
        local_domain,
    );
    activity["@context"] =
        serde_json::Value::String("https://www.w3.org/ns/activitystreams".to_string());
    activity["published"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
    activity["to"] = serde_json::json!(targets.to);
    if !targets.is_dm {
        activity["cc"] = serde_json::json!([addr.followers_uri]);
    }

    fan_out_activity(
        ap_client,
        &targets.inboxes,
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
    let targets = match resolve_reaction_targets(db, post_id, actor_id).await? {
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
        &targets.object_ap_id,
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
    let mut activity =
        build_undo_reaction_activity(&addr, &undo_id, &chrono::Utc::now().to_rfc3339(), inner);
    activity["to"] = serde_json::json!(targets.to);
    // `build_undo_reaction_activity`はデフォルトで`cc: [followers_uri]`を持つため、
    // DM宛の場合は明示的に消す（フォロワーへ漏らさないため、通常投稿宛はそのまま残す）。
    if targets.is_dm {
        activity.as_object_mut().unwrap().remove("cc");
    }

    fan_out_activity(
        ap_client,
        &targets.inboxes,
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
