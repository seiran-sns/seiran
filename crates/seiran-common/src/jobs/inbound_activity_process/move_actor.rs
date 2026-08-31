use super::*;


/// Move（アカウント引っ越し）を受信する（第1段階: 受信処理のみ、送信側=引っ越し実行UIは未実装）。
///
/// Mastodon 等の実装慣習に合わせ、`actor`（`object`と同一の移転元本人）から`target`
/// （移転先）への引っ越しとして扱う。なりすまし対策として、`target`アクター文書の
/// `alsoKnownAs`に`actor`のURIが含まれていることを確認できた場合のみ処理する
/// （移転先が同意していない引っ越しでフォロワーを乗っ取られることを防ぐ）。
///
/// 移転元をフォローしていた（フォロー申請中も含む）ローカルアクター全員について、
/// 移転先へのフォローを送り直す。この「フォロワー」には実ユーザーだけでなく、
/// リスト機能の list-relay プロキシアクター（`system_actor`）も含まれるため、
/// 移転元をリストに入れていた場合の付け替えも同じループで自然にカバーされる
/// （`follows`テーブルへの登録経路が実ユーザーもプロキシアクターも同じため）。
/// 加えて `list_members` 側のメンバー行自体も移転先へ差し替える。
pub(super) async fn handle_move(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let old_actor_uri = activity["actor"]
        .as_str()
        .ok_or("Move: actor フィールドがありません")?;
    // object は actor 自身を指すのが仕様（Mastodon実装）。異なる場合はなりすまし
    // または実装違いの疑いがあるため処理しない。
    let object_uri = activity["object"]
        .as_str()
        .or_else(|| activity["object"]["id"].as_str());
    if let Some(object_uri) = object_uri {
        if object_uri != old_actor_uri {
            return Err(format!(
                "Move: actor({}) と object({}) が一致しません",
                old_actor_uri, object_uri
            ));
        }
    }
    let target_uri = activity["target"]
        .as_str()
        .ok_or("Move: target フィールドがありません")?;
    tracing::info!(
        "[Move] 受信: actor={} object={:?} target={}",
        old_actor_uri,
        object_uri,
        target_uri
    );

    // 移転元がローカルDBに未登録（誰もフォロー・リスト登録していない）なら、
    // 移行すべき関係が無いため何もしない。
    let Some(old_actor) = inbox
        .actor_repo
        .find_by_ap_uri(old_actor_uri)
        .await
        .map_err(|e| format!("移転元アクター検索エラー: {}", e))?
    else {
        tracing::info!("[Move] 移転元 {} は未知のため無視します", old_actor_uri);
        return Ok(());
    };
    tracing::info!(
        "[Move] 移転元 {} を actor_id={} として解決",
        old_actor_uri,
        old_actor.id
    );

    // なりすまし対策: target アクター文書の alsoKnownAs に移転元URIが含まれることを
    // 確認できた場合のみ処理する。恒久的に検証を通らないケース（移転先が未承認）は
    // リトライしても解決しないため、エラーにはせずログのみで無視する。
    let signing_key = super::reference::system_signing_key(inbox);
    let target_ap = match ap_client
        .fetch_actor_signed(target_uri, (&signing_key.0, &signing_key.1))
        .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("[Move] 移転先 {} の取得に失敗: {}", target_uri, e);
            return Ok(());
        }
    };
    tracing::info!(
        "[Move] 移転先 {} の alsoKnownAs={:?}",
        target_uri,
        target_ap.also_known_as
    );
    if !target_ap.claims_also_known_as(old_actor_uri) {
        tracing::warn!(
            "[Move] 移転先 {} の alsoKnownAs に移転元 {} が含まれていないため無視します",
            target_uri,
            old_actor_uri
        );
        return Ok(());
    }

    let new_actor = upsert_remote_fedi_actor(inbox, ap_client, target_uri).await?;
    tracing::info!(
        "[Move] 移転先 {} を actor_id={} (inbox={}) として解決",
        target_uri,
        new_actor.actor_id,
        new_actor.inbox
    );
    if new_actor.actor_id == old_actor.id {
        // 自分自身への Move（既に処理済み、またはURIの揺れ）。何もしない。
        return Ok(());
    }
    if new_actor.inbox.is_empty() {
        return Err("Move: 移転先アクターの inbox が取得できません".to_string());
    }

    let followers = inbox
        .follow_repo
        .find_all_local_followers_with_status(old_actor.id)
        .await
        .map_err(|e| format!("followers 検索エラー: {}", e))?;
    tracing::info!(
        "[Move] 移転元 actor_id={} のローカルフォロワー: {:?}",
        old_actor.id,
        followers
    );

    for (follower_actor_id, _old_status) in followers {
        if follower_actor_id == new_actor.actor_id {
            continue;
        }
        if let Err(e) = migrate_one_follow(
            inbox,
            ap_client,
            follower_actor_id,
            &old_actor,
            &new_actor,
            target_uri,
        )
        .await
        {
            tracing::error!(
                "[Move] follower={} の付け替えに失敗: {}",
                follower_actor_id,
                e
            );
        }
    }

    // リストのメンバーシップも移転先へ差し替える（対応するAP側フォロー処理は上の
    // ループでlist-relayプロキシアクター分として既に完了している）。
    let list_ids = inbox
        .list_repo
        .list_ids_containing_actor(old_actor.id)
        .await
        .map_err(|e| format!("リスト検索エラー: {}", e))?;
    let now = chrono::Utc::now();
    for list_id in list_ids {
        if let Err(e) = inbox.list_repo.remove_member(list_id, old_actor.id).await {
            tracing::error!("[Move] list={} のメンバー削除に失敗: {}", list_id, e);
            continue;
        }
        if let Err(e) = inbox
            .list_repo
            .add_member(list_id, new_actor.actor_id, now)
            .await
        {
            tracing::error!("[Move] list={} のメンバー追加に失敗: {}", list_id, e);
        }
    }

    tracing::info!("[Move] {} → {} 引っ越し処理完了", old_actor_uri, target_uri);
    Ok(())
}

/// Move受信時、1人のローカルフォロワー（実ユーザーまたはlist-relayプロキシアクター）の
/// フォロー関係を移転元(`old_actor`)から移転先(`new_actor`)へ付け替える。
/// 実ユーザー（`actors.user_id`が`Some`）にのみ、結果に応じた独自通知
/// （`MoveRefollowed`/`MoveAlreadyFollowing`）を送る（システムアクターには送らない）。
async fn migrate_one_follow(
    inbox: &InboxContext,
    ap_client: &ApClient,
    follower_actor_id: i64,
    old_actor: &Actor,
    new_actor: &RemoteActorInfo,
    new_actor_uri: &str,
) -> Result<(), String> {
    let follower = inbox
        .actor_repo
        .find_by_id(follower_actor_id)
        .await
        .map_err(|e| format!("フォロワーアクター取得エラー: {}", e))?
        .ok_or_else(|| {
            format!(
                "フォロワーアクター(id={})が見つかりません",
                follower_actor_id
            )
        })?;

    let already_status = inbox
        .follow_repo
        .find_status(follower_actor_id, new_actor.actor_id)
        .await
        .map_err(|e| format!("フォロー状態取得エラー: {}", e))?;
    tracing::info!(
        "[Move] follower={}({}) の新フォロー先(actor_id={})への既存status={:?}",
        follower_actor_id,
        follower.username,
        new_actor.actor_id,
        already_status
    );

    if already_status.is_some() {
        inbox
            .follow_repo
            .delete_by_actors(follower_actor_id, old_actor.id)
            .await
            .map_err(|e| format!("旧フォロー削除エラー: {}", e))?;
        notify_move(
            inbox,
            &follower,
            old_actor,
            new_actor.actor_id,
            NotificationKind::MoveAlreadyFollowing,
            "moveAlreadyFollowing",
        )
        .await;
        tracing::info!(
            "[Move] follower={}({}) は既に移転先をフォロー済みのため旧フォローのみ削除しました",
            follower_actor_id,
            follower.username
        );
        return Ok(());
    }

    // Follow は当該フォロワー自身の身元（実ユーザー or list-relayプロキシアクター）で
    // 送る（`handlers::follows::follow_fedi`・`jobs::proxy_follow_sync`と同じ組み立て方）。
    let follower_uri = format!("https://{}/users/{}", inbox.local_domain, follower.username);
    let actor_key_id = format!("{}#main-key", follower_uri);
    let follow_id = format!(
        "https://{}/activities/follow/{}-{}",
        inbox.local_domain, follower_actor_id, new_actor.actor_id
    );
    let follow_activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Follow",
        "id": follow_id,
        "actor": follower_uri,
        "object": new_actor_uri,
    });
    let body =
        serde_json::to_string(&follow_activity).map_err(|e| format!("JSON構築エラー: {}", e))?;

    ap_client
        .sign_and_post(
            &new_actor.inbox,
            &body,
            &actor_key_id,
            &inbox.ap_private_key_pem,
        )
        .await?;

    inbox
        .follow_repo
        .delete_by_actors(follower_actor_id, old_actor.id)
        .await
        .map_err(|e| format!("旧フォロー削除エラー: {}", e))?;
    inbox
        .follow_repo
        .upsert_pending(follower_actor_id, new_actor.actor_id)
        .await
        .map_err(|e| format!("新フォローINSERTエラー: {}", e))?;

    notify_move(
        inbox,
        &follower,
        old_actor,
        new_actor.actor_id,
        NotificationKind::MoveRefollowed,
        "moveRefollowed",
    )
    .await;

    tracing::info!(
        "[Move] {} → {} 付け替えFollow送信完了 (pending)",
        follower_uri,
        new_actor_uri
    );
    Ok(())
}

/// Move付け替え結果の通知（独自拡張）。`recipient`がシステムアクター（list-relay等、
/// `user_id`が`None`）の場合は表示先が無いため送らない。
async fn notify_move(
    inbox: &InboxContext,
    recipient: &Actor,
    old_actor: &Actor,
    new_actor_id: i64,
    kind: NotificationKind,
    event_type: &'static str,
) {
    if recipient.user_id.is_none() {
        tracing::info!(
            "[Move] recipient={}({}) はシステムアクターのため通知をスキップします",
            recipient.id,
            recipient.username
        );
        return;
    }
    inbox.stream_hub.publish_event(
        HashSet::from([recipient.id]),
        event_type,
        serde_json::json!({}),
    );
    let notif_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = inbox
        .notification_repo
        .insert(
            notif_id,
            recipient.id,
            kind,
            Some(old_actor.id),
            None,
            None,
            None,
            None,
            None,
            Some(new_actor_id),
        )
        .await
    {
        tracing::error!("[Move] notifications INSERT 失敗: {}", e);
    }
}
