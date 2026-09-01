use super::*;

// Follow アクティビティを処理し Accept を送信する
pub(super) async fn handle_follow(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let follower_uri = activity["actor"]
        .as_str()
        .ok_or("Follow: actor フィールドがありません")?;
    let target_uri = activity["object"]
        .as_str()
        .ok_or("Follow: object フィールドがありません")?;

    // target_uri から "https://{local_domain}/users/{username}" のユーザー名を抽出。
    // ホスト名の一致まで確認しないと、リモートの同名ユーザー（例:
    // https://fedibird.com/users/momozou）宛の Follow をローカルの同名ユーザーへの
    // Follow と誤認してしまう（末尾セグメントだけを見る rsplit('/') はドメインを見ない）。
    let local_username = crate::ap::extract_local_username(target_uri, &inbox.local_domain)
        .ok_or("Follow: object URI が自ドメインのアクターを指していません")?;

    // ローカルアクターが実在するか確認
    let local_actor = inbox
        .actor_repo
        .find_by_username_domain(local_username, &inbox.local_domain)
        .await
        .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("ローカルアクター '{}' が存在しません", local_username))?;
    if local_actor.actor_type != "local" {
        return Err(format!(
            "'{}' はローカルアクターではありません",
            local_username
        ));
    }
    let local_actor_id = local_actor.id;

    // リモートアクターを解決・upsert（inbox URL・display_name・アバター用）
    let remote = upsert_remote_fedi_actor(inbox, ap_client, follower_uri).await?;
    if remote.inbox.is_empty() {
        return Err("Follow: リモートアクターの inbox が取得できません".to_string());
    }
    let follower_actor_id = remote.actor_id;

    // ブロック済みチェック（Fedi標準の片方向拒否ブロック）: こちらが相手をブロック中なら、
    // Accept を送らずサイレントに無視する（フォロー関係も作らない）。
    let (is_blocking, _) = inbox
        .block_repo
        .find_relationship(local_actor_id, follower_actor_id)
        .await
        .map_err(|e| format!("ブロック関係取得エラー: {}", e))?;
    if is_blocking {
        tracing::info!(
            "[Follow] {} は '{}' にブロックされているため無視します（Accept送信なし）",
            follower_uri,
            local_username
        );
        return Ok(());
    }

    // follows テーブルに挿入（重複時はスキップ、リモートからのフォローは自動 accepted）
    inbox
        .follow_repo
        .insert_accepted(follower_actor_id, local_actor_id)
        .await
        .map_err(|e| format!("follows INSERT エラー: {}", e))?;

    // リアルタイム通知（#37）: フォローされたローカルユーザーへ
    inbox.stream_hub.publish_event(
        HashSet::from([local_actor_id]),
        "follow",
        serde_json::json!({
            "actor": { "username": remote.username, "domain": remote.domain, "displayName": remote.display_name },
        }),
    );
    let notif_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = inbox
        .notification_repo
        .insert(
            notif_id,
            local_actor_id,
            NotificationKind::Follow,
            Some(follower_actor_id),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
    {
        tracing::error!("[Follow] notifications INSERT 失敗: {}", e);
    }

    // Accept アクティビティを構築して送信
    let local_actor_uri = format!("https://{}/users/{}", inbox.local_domain, local_username);
    let accept_id = format!(
        "https://{}/accepts/{}",
        inbox.local_domain,
        generate_snowflake_id(chrono::Utc::now())
    );
    let actor_key_id = format!("{}#main-key", local_actor_uri);

    let accept = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Accept",
        "id": accept_id,
        "actor": local_actor_uri,
        "object": activity
    });
    let accept_body =
        serde_json::to_string(&accept).map_err(|e| format!("Accept シリアライズ失敗: {}", e))?;

    ap_client
        .sign_and_post(
            &remote.inbox,
            &accept_body,
            &actor_key_id,
            &inbox.ap_private_key_pem,
        )
        .await?;

    tracing::info!(
        "[Follow] {} → {} フォロー完了・Accept 送信済み",
        follower_uri,
        local_actor_uri
    );
    Ok(())
}

// Accept(Follow) を受け取り follows.status を accepted に更新する
pub(super) async fn handle_accept(
    activity: serde_json::Value,
    inbox: &InboxContext,
) -> Result<(), String> {
    let obj = &activity["object"];
    let remote_actor_uri = activity["actor"]
        .as_str()
        .ok_or("Accept: actor がありません")?;

    // Mitra などは Accept.object に Follow オブジェクトではなく、その URI を返す。
    // URI 形式には送信元・送信先の actor ID を含め、署名主体である Accept.actor と
    // 送信先が一致することを後段で検証する。
    let local_actor_id_from_uri = obj
        .as_str()
        .and_then(|uri| parse_local_follow_activity_id(uri, &inbox.local_domain));

    let local_actor = if let Some((local_actor_id, expected_remote_actor_id)) =
        local_actor_id_from_uri
    {
        let remote_actor = inbox
            .actor_repo
            .find_by_ap_uri(remote_actor_uri)
            .await
            .map_err(|e| format!("リモートアクター検索エラー: {}", e))?
            .ok_or_else(|| {
                format!(
                    "リモートアクター '{}' が DB に見つかりません",
                    remote_actor_uri
                )
            })?;
        if remote_actor.id != expected_remote_actor_id {
            return Err("Accept: actor が Follow Activity の送信先と一致しません".to_string());
        }
        inbox
            .actor_repo
            .find_by_id(local_actor_id)
            .await
            .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
            .ok_or_else(|| format!("ローカルアクター ID '{}' が見つかりません", local_actor_id))?
    } else {
        if obj["type"].as_str() != Some("Follow") {
            return Ok(());
        }
        let local_actor_uri = obj["actor"]
            .as_str()
            .ok_or("Accept/Follow: object.actor がありません")?;

        // 埋め込み Follow 形式との後方互換性を維持する。
        let suffix = format!("https://{}/users/", inbox.local_domain);
        let local_username = local_actor_uri
            .strip_prefix(&suffix)
            .ok_or("Accept: object.actor がローカルアクターではありません")?;
        inbox
            .actor_repo
            .find_by_username_domain(local_username, &inbox.local_domain)
            .await
            .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
            .ok_or_else(|| format!("ローカルアクター '{}' が見つかりません", local_username))?
    };
    if local_actor.actor_type != "local" {
        return Err(format!(
            "actor ID '{}' はローカルアクターではありません",
            local_actor.id
        ));
    }
    let local_actor_id = local_actor.id;
    let local_actor_uri = format!(
        "https://{}/users/{}",
        inbox.local_domain, local_actor.username
    );

    // リモートアクターを ap_uri から特定
    let remote_actor = inbox
        .actor_repo
        .find_by_ap_uri(remote_actor_uri)
        .await
        .map_err(|e| format!("リモートアクター検索エラー: {}", e))?
        .ok_or_else(|| {
            format!(
                "リモートアクター '{}' が DB に見つかりません",
                remote_actor_uri
            )
        })?;
    let remote_actor_id = remote_actor.id;

    // follows.status を accepted に更新
    let rows = inbox
        .follow_repo
        .accept(local_actor_id, remote_actor_id)
        .await
        .map_err(|e| format!("follows UPDATE エラー: {}", e))?;

    tracing::info!(
        "[Accept] {} → {} フォロー確定 (rows={})",
        local_actor_uri,
        remote_actor_uri,
        rows
    );

    // リアルタイム通知（#37）: フォローが承諾されたローカルユーザーへ
    if rows > 0 {
        inbox.stream_hub.publish_event(
            HashSet::from([local_actor_id]),
            "followAccepted",
            serde_json::json!({
                "actor": {
                    "username": remote_actor.username,
                    "domain": remote_actor.domain,
                    "displayName": remote_actor.display_name,
                },
            }),
        );
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = inbox
            .notification_repo
            .insert(
                notif_id,
                local_actor_id,
                NotificationKind::FollowRequestAccepted,
                Some(remote_actor.id),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
        {
            tracing::error!("[Accept] notifications INSERT 失敗: {}", e);
        }
    }
    Ok(())
}

pub(super) fn parse_local_follow_activity_id(uri: &str, local_domain: &str) -> Option<(i64, i64)> {
    let ids = uri.strip_prefix(&format!("https://{}/activities/follow/", local_domain))?;
    let (local_actor_id, remote_actor_id) = ids.split_once('-')?;
    Some((local_actor_id.parse().ok()?, remote_actor_id.parse().ok()?))
}

#[cfg(test)]
mod follow_accept_tests {
    use super::parse_local_follow_activity_id;

    #[test]
    fn parses_local_and_remote_actor_ids() {
        assert_eq!(
            parse_local_follow_activity_id(
                "https://seiran.example/activities/follow/123-456",
                "seiran.example",
            ),
            Some((123, 456))
        );
    }

    #[test]
    fn rejects_foreign_or_legacy_follow_activity_ids() {
        assert_eq!(
            parse_local_follow_activity_id(
                "https://other.example/activities/follow/123-456",
                "seiran.example",
            ),
            None
        );
        assert_eq!(
            parse_local_follow_activity_id(
                "https://seiran.example/activities/follow/456",
                "seiran.example",
            ),
            None
        );
    }
}
