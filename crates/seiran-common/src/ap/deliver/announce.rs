use super::*;
use super::infra::*;
use super::activity::*;


/// Announce 対象の元ポストが Fedi リモートである場合、その投稿者の inbox URL と actor URI を返す。
/// リアクション配送（`resolve_reaction_targets`）と同様、フォロワー配送だけでは元投稿者の
/// サーバーに Announce が届かず、ブースト数の反映や通知が発生しないため必要。
async fn resolve_announce_object_actor(
    db: &PgPool,
    original_ap_object_id: &str,
) -> Result<Option<(String, String)>, ApError> {
    let row = sqlx::query(
        "SELECT a.ap_inbox_url, a.ap_uri
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.ap_object_id = $1 AND a.actor_type = 'fedi' LIMIT 1",
    )
    .bind(original_ap_object_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("Announce対象ポスト取得エラー: {}", e)))?;

    let Some(row) = row else {
        return Ok(None);
    };
    let inbox: Option<String> = row.try_get("ap_inbox_url").unwrap_or(None);
    let actor_uri: Option<String> = row.try_get("ap_uri").unwrap_or(None);
    Ok(inbox.zip(actor_uri))
}

/// ローカルアクターの AP Announce アクティビティを Fedi フォロワー全員 + 元ポストの投稿者へ配送する
///
/// `original_ap_object_id` は Announce の対象（元ポストの AP URI）。
pub async fn deliver_ap_announce(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    original_ap_object_id: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let visibility: String = sqlx::query_scalar("SELECT visibility::text FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_optional(db)
        .await
        .map_err(|e| ApError::Other(e.to_string()))?
        .unwrap_or_else(|| "public".to_string());
    let object_actor = resolve_announce_object_actor(db, original_ap_object_id).await?;

    let mut inboxes: std::collections::HashSet<String> = fetch_fedi_follower_inboxes(db, actor_id)
        .await?
        .into_iter()
        .collect();
    if let Some((inbox, _)) = &object_actor {
        inboxes.insert(inbox.clone());
    }
    let inboxes: Vec<String> = inboxes.into_iter().collect();

    let addr = local_actor_address(local_domain, &username);
    let announce_id = format!("https://{}/announces/{}", local_domain, post_id);
    let mut activity = build_announce_activity(
        &addr,
        &announce_id,
        original_ap_object_id,
        &chrono::Utc::now().to_rfc3339(),
        &visibility,
    );
    // 元投稿者を明示的に cc へ含める（Mastodon 互換）。フォロワー配送だけでは
    // 相手サーバーがブースト数・通知に反映しないことがあるため。
    if let Some((_, actor_uri)) = &object_actor {
        if let Some(cc) = activity.get_mut("cc").and_then(|v| v.as_array_mut()) {
            if !cc.iter().any(|v| v.as_str() == Some(actor_uri.as_str())) {
                cc.push(serde_json::Value::String(actor_uri.clone()));
            }
        }
    }

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!("Announce post_id={} username={}", post_id, username),
    )
    .await
}

/// ローカルアクターの AP Undo(Announce) を Fedi フォロワー全員 + 元ポストの投稿者へ配送する。
/// `announce_post_id` はリポスト投稿の posts.id、`original_ap_object_id` は元ポストの AP URI。
pub async fn deliver_undo_announce(
    ap_client: &ApClient,
    db: &PgPool,
    announce_post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    original_ap_object_id: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let object_actor = resolve_announce_object_actor(db, original_ap_object_id).await?;
    let mut inboxes: std::collections::HashSet<String> = fetch_fedi_follower_inboxes(db, actor_id)
        .await?
        .into_iter()
        .collect();
    if let Some((inbox, _)) = &object_actor {
        inboxes.insert(inbox.clone());
    }
    let inboxes: Vec<String> = inboxes.into_iter().collect();

    let addr = local_actor_address(local_domain, &username);
    let announce_id = format!("https://{}/announces/{}", local_domain, announce_post_id);
    let undo_id = format!("https://{}/undos/{}", local_domain, announce_post_id);
    let activity = build_undo_announce_activity(
        &addr,
        &undo_id,
        &announce_id,
        original_ap_object_id,
        &chrono::Utc::now().to_rfc3339(),
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Undo(Announce) post_id={} username={}",
            announce_post_id, username
        ),
    )
    .await
}
