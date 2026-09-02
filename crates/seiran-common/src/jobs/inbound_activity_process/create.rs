use super::note_input::extract_mentioned_local_usernames;
use super::note_save::{save_ap_note_core, SaveApNoteOutcome};
use super::reference::ReferenceResolutionMode;
use super::*;

/// リモート投稿に起因するローカルユーザーへの通知（引用・リプライ・メンション）を、
/// リアルタイムイベント配信と通知レコード挿入の対で作る。3 種で完全に同形のため共通化する
/// （how: 通知の生成・配信）。呼び出し側は「誰に・どの種別で」だけを決める（what）。
pub(super) async fn notify_local_actor(
    inbox: &InboxContext,
    target_actor_id: i64,
    kind: NotificationKind,
    event_name: &str,
    from_actor_id: i64,
    post_id: i64,
    remote: &RemoteActorInfo,
) {
    inbox.stream_hub.publish_event(
        HashSet::from([target_actor_id]),
        event_name,
        serde_json::json!({
            "postId": post_id.to_string(),
            "actor": { "username": remote.username, "domain": remote.domain, "displayName": remote.display_name },
        }),
    );
    let notif_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = inbox
        .notification_repo
        .insert(
            notif_id,
            target_actor_id,
            kind,
            Some(from_actor_id),
            Some(post_id),
            None,
            None,
            None,
            None,
            None,
        )
        .await
    {
        tracing::error!(
            "[Create/Note] {} notification INSERT 失敗: {}",
            event_name,
            e
        );
    }
}

/// `broadcast_created_note` の入力。保存済みリモート投稿を WebSocket 配信するのに必要な値だけを束ねる。
pub(super) struct CreatedNoteContext<'a> {
    pub post_id: i64,
    pub body: &'a str,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub actor_id: i64,
    pub remote: &'a RemoteActorInfo,
    pub emoji_map: &'a serde_json::Value,
    pub visibility: &'a str,
    pub reply_to_post_id: Option<i64>,
    pub recipient_actor_ids: &'a [i64],
}

/// 保存済みリモート投稿を可視性に応じて WebSocket 配信する（how: リアルタイム配信）。
/// direct は宛先のみ（本文漏洩防止のためフォロワーには配信しない）、それ以外は
/// タイムライン系チャンネル（home/local/hybrid/global/userList/hashtag）購読者へ流す。
pub(super) async fn broadcast_created_note(inbox: &InboxContext, ctx: CreatedNoteContext<'_>) {
    let note_json = serde_json::json!({
        "id": ctx.post_id.to_string(),
        "text": ctx.body,
        "createdAt": ctx.created_at.to_rfc3339(),
        "user": {
            "id": ctx.actor_id,
            "username": ctx.remote.username,
            "domain": ctx.remote.domain,
            "displayName": ctx.remote.display_name,
            "actorType": "fedi",
            "avatarUrl": ctx.remote.avatar_url,
        },
        "attachments": [],
        "emojis": ctx.emoji_map,
    });
    if ctx.visibility == "direct" {
        let recipients: HashSet<i64> = ctx.recipient_actor_ids.iter().copied().collect();
        if !recipients.is_empty() {
            let mut note_json = note_json;
            note_json["visibility"] = serde_json::json!("direct");
            inbox.stream_hub.publish_note(recipients, &note_json);
        }
    } else {
        let mut home_recipients: HashSet<i64> = inbox
            .follow_repo
            .find_home_recipient_ids(ctx.actor_id, ctx.reply_to_post_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        home_recipients.insert(ctx.actor_id);
        let list_ids: HashSet<i64> = inbox
            .list_repo
            .list_ids_containing_actor(ctx.actor_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        let hashtags: HashSet<String> = crate::hashtag::extract_hashtags(ctx.body)
            .into_iter()
            .collect();
        let scope = ChannelScope {
            is_local: false,
            visibility: ctx.visibility.to_string(),
            home_recipients: Arc::new(home_recipients),
            list_ids: Arc::new(list_ids),
            hashtags: Arc::new(hashtags),
        };
        inbox.stream_hub.publish_channel_note(scope, note_json);
    }
}

// Create(Note) を受け取り posts テーブルに保存する。DB保存の中核処理は`save_ap_note_core`
// （参照解決経由の`save_fetched_remote_note`と共通）に委譲し、ここではCreate直接受信
// 特有の後処理（通知生成・WebSocket配信）のみを行う。
pub(super) async fn handle_create_note(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let note = &activity["object"];
    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("Create: actor がありません")?;

    let outcome = save_ap_note_core(
        note,
        actor_uri,
        inbox,
        ap_client,
        ReferenceResolutionMode::OneHopFetch,
    )
    .await?;

    let saved = match outcome {
        SaveApNoteOutcome::AlreadyExists { .. } => return Ok(()),
        SaveApNoteOutcome::Inserted(saved) => saved,
    };

    // 引用通知: リモート Fedi ユーザーがローカルユーザーの投稿を引用した場合に作る。
    if let Some(quoted_post_id) = saved.quote_of_post_id {
        match inbox.post_repo.find_delivery_meta(quoted_post_id).await {
            Ok(Some(meta)) if meta.actor_type == "local" && meta.actor_id != saved.actor_id => {
                notify_local_actor(
                    inbox,
                    meta.actor_id,
                    NotificationKind::Quote,
                    "quote",
                    saved.actor_id,
                    saved.post_id,
                    &saved.remote,
                )
                .await;
            }
            Ok(_) => {}
            Err(e) => tracing::error!("[Create/Note] 引用元メタ情報の取得に失敗: {}", e),
        }
    }

    // リプライ通知: リプライ先がローカルユーザーの投稿であれば通知を作る（自己リプライは除く）。
    let reply_parent_local_actor_id: Option<i64> = match saved.reply_to_post_id {
        Some(parent_id) => inbox
            .post_repo
            .find_delivery_meta(parent_id)
            .await
            .ok()
            .flatten()
            .filter(|m| m.actor_type == "local")
            .map(|m| m.actor_id),
        None => None,
    };
    if let Some(parent_actor_id) = reply_parent_local_actor_id.filter(|id| *id != saved.actor_id) {
        notify_local_actor(
            inbox,
            parent_actor_id,
            NotificationKind::Reply,
            "reply",
            saved.actor_id,
            saved.post_id,
            &saved.remote,
        )
        .await;
    }

    // メンション通知: `tag[]` の `Mention` がローカルユーザーの AP actor URI
    // （`https://{local_domain}/users/{username}`）を指す場合、通知を作る。
    // ローカルユーザーの `ap_uri` は動的組み立てのため、DM宛先解決と同じ
    // `extract_local_username` でホスト名まで検証してから解決する（他インスタンスの
    // 同名ユーザーを誤って拾わないため。詳細は下のテスト参照）。
    let mut mentioned_local_actor_ids: Vec<i64> = Vec::new();
    for local_username in extract_mentioned_local_usernames(&saved.tags, &inbox.local_domain) {
        if let Ok(Some(actor)) = inbox
            .actor_repo
            .find_by_username_domain(local_username, &inbox.local_domain)
            .await
        {
            if actor.actor_type == "local" && !mentioned_local_actor_ids.contains(&actor.id) {
                mentioned_local_actor_ids.push(actor.id);
            }
        }
    }
    for mentioned_actor_id in mentioned_local_actor_ids {
        notify_local_actor(
            inbox,
            mentioned_actor_id,
            NotificationKind::Mention,
            "mention",
            saved.actor_id,
            saved.post_id,
            &saved.remote,
        )
        .await;
    }

    // WebSocket リアルタイム配信。
    broadcast_created_note(
        inbox,
        CreatedNoteContext {
            post_id: saved.post_id,
            body: &saved.body,
            created_at: saved.created_at,
            actor_id: saved.actor_id,
            remote: &saved.remote,
            emoji_map: &saved.emoji_map,
            visibility: saved.visibility,
            reply_to_post_id: saved.reply_to_post_id,
            recipient_actor_ids: &saved.recipient_actor_ids,
        },
    )
    .await;

    let dup_info = saved
        .parent_original_post_id
        .map_or(String::new(), |id| format!(" (parent_original={})", id));
    tracing::info!(
        "[Create/Note] {} から投稿を受信・保存: {}{}",
        actor_uri,
        saved.note_id,
        dup_info
    );
    Ok(())
}
