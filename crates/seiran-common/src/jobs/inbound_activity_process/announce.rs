use super::reference::{resolve_reference, RefStatus};
use super::*;

// Announce(Note) を受け取り posts テーブルに保存する
pub(super) async fn handle_announce(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let announce_id = activity["id"].as_str().ok_or("Announce: id がありません")?;
    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("Announce: actor がありません")?;
    let object_uri = activity["object"]
        .as_str()
        .ok_or("Announce: object がありません")?;
    let published = activity["published"].as_str().unwrap_or("");
    // Announce（リポスト）自身の to/cc から可視性を判定する（元ポストの可視性ではなく、
    // このリポストという行為自体が公開/フォロワー限定/ひかえめのいずれで行われたか）。
    let visibility = classify_ap_visibility(
        &as_string_list(&activity["to"]),
        &as_string_list(&activity["cc"]),
    );

    // 同一Announceの再配送では、リポスト行を二重生成しない。
    // repost_of_post_idが未解決（pending/gone）の場合はこの後の重複チェック
    // （同一アクター×同一対象での重複）が機能しないため、ap_object_id自体での
    // 早期dedupが必須になる。
    if inbox
        .post_repo
        .find_id_by_ap_or_at_uri(announce_id)
        .await
        .map_err(|e| format!("Announce 重複チェック失敗: {}", e))?
        .is_some()
    {
        return Ok(());
    }

    // 公開日時を parse して snowflake ID を生成
    let created_at = published
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap_or_else(|_| chrono::Utc::now());
    let post_id = generate_snowflake_id(created_at);

    // リモートアクターを解決・upsert（未登録なら作成）
    let remote = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let actor_id = remote.actor_id;

    // リポスト対象をDBから検索し、無ければ1段階だけフェッチを試みる（#231）。
    // フェッチが404/410（gone）または一時的失敗（pending）でも、リポストの箱
    // （wrapper post行）自体は必ず保存する（「対象が見当たらないが何かをリポストした」
    // という表示を可能にするため。空リプ文化への配慮）。
    let (repost_of_post_id, repost_of_ap_uri, repost_of_ref_status) =
        resolve_reference(Some(object_uri), inbox, ap_client)
            .await
            .into_parts();

    // 重複チェック（同一アクターによる同一ポストのリポスト）。対象が未解決の場合は
    // 判定不能なため、上のannounce_id早期dedupだけに委ねる。
    if let Some(target_id) = repost_of_post_id {
        if inbox
            .post_repo
            .find_repost_undo_info(actor_id, target_id)
            .await
            .map_err(|e| format!("重複チェック失敗: {}", e))?
            .is_some()
        {
            return Ok(());
        }
    }

    // リポストをDBに挿入
    inbox
        .post_repo
        .insert_repost(InsertRepostParams {
            id: post_id,
            actor_id,
            ap_object_id: announce_id,
            repost_of_post_id,
            repost_of_ap_uri: repost_of_ap_uri.as_deref(),
            repost_of_ref_status: repost_of_ref_status.map(RefStatus::as_db_str),
            created_at,
            visibility,
        })
        .await
        .map_err(|e| format!("リポスト挿入失敗: {}", e))?;

    // リポスト通知: リモート Fedi ユーザーがローカルユーザーの投稿をリポストした場合に作る。
    // 対象が未解決の場合はローカル投稿かどうか判定できないため通知しない。
    if let Some(target_id) = repost_of_post_id {
        match inbox.post_repo.find_delivery_meta(target_id).await {
            Ok(Some(meta)) if meta.actor_type == "local" && meta.actor_id != actor_id => {
                inbox.stream_hub.publish_event(
                    HashSet::from([meta.actor_id]),
                    "repost",
                    serde_json::json!({
                        "postId": post_id.to_string(),
                        "actor": {
                            "username": remote.username,
                            "domain": remote.domain,
                            "displayName": remote.display_name
                        },
                    }),
                );
                let notif_id = generate_snowflake_id(chrono::Utc::now());
                if let Err(e) = inbox
                    .notification_repo
                    .insert(
                        notif_id,
                        meta.actor_id,
                        NotificationKind::Repost,
                        Some(actor_id),
                        Some(post_id),
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                {
                    tracing::error!("[Inbox/Announce] repost notifications INSERT 失敗: {}", e);
                }
            }
            Ok(_) => {}
            Err(e) => tracing::error!("[Inbox/Announce] 元ポストメタ情報の取得に失敗: {}", e),
        }
    }

    tracing::info!(
        "[Inbox/Announce] リポスト保存完了: id={}, actor_id={}, repost_of={:?}",
        post_id,
        actor_id,
        repost_of_post_id
    );

    Ok(())
}
