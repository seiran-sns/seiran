use super::*;

/// Accept/Reject/Undo の `object`（Follow activity。文字列URIまたは`{"id": ...}`形式の
/// どちらでも来うる）から Follow activity の id を取り出し、`fediverse_relays` に一致する
/// レコードがあれば `relay_id` を返す。一致しなければ通常のローカルフォロー応答とみなす。
pub(super) async fn relay_id_for_follow_object(
    activity: &serde_json::Value,
    inbox: &InboxContext,
) -> Result<Option<i64>, String> {
    let object = &activity["object"];
    let follow_activity_id = match object.as_str() {
        Some(s) => Some(s.to_string()),
        None => object["id"].as_str().map(|s| s.to_string()),
    };
    let Some(follow_activity_id) = follow_activity_id else {
        return Ok(None);
    };

    let relays = PgRelayRepository::new(inbox.db_pool.clone());
    let relay = relays
        .find_by_follow_activity_id(&follow_activity_id)
        .await
        .map_err(|e| format!("リレー検索失敗: {}", e))?;
    let Some(relay) = relay else {
        return Ok(None);
    };
    let response_actor = activity["actor"]
        .as_str()
        .ok_or_else(|| "リレー応答にactorがありません".to_string())?;
    let response_actor_url = url::Url::parse(response_actor)
        .map_err(|_| "リレー応答actorが不正なURLです".to_string())?;
    let relay_inbox_url =
        url::Url::parse(&relay.inbox_url).map_err(|_| "登録リレーURLが不正です".to_string())?;
    if response_actor_url.origin() != relay_inbox_url.origin() {
        return Err(format!(
            "リレー応答actorが登録inboxと同一originではありません: {}",
            response_actor
        ));
    }
    Ok(Some(relay.id))
}

/// リレーが Follow を Accept した。配送対象（status='accepted'）にする。
pub(super) async fn handle_relay_accept(relay_id: i64, inbox: &InboxContext) -> Result<(), String> {
    let relays = PgRelayRepository::new(inbox.db_pool.clone());
    relays
        .update_status(relay_id, RelayStatus::Accepted, None)
        .await
        .map_err(|e| format!("fediverse_relays UPDATE失敗: {}", e))?;
    tracing::info!(
        "[Job::InboundActivityProcess] リレー(id={}) Accept受信",
        relay_id
    );
    Ok(())
}

/// リレーが Follow を Reject した、またはリレー側から Undo(Follow) が届いた。
/// 配送対象から外す（レコード自体は管理者が気づけるよう残す）。
pub(super) async fn handle_relay_reject(relay_id: i64, inbox: &InboxContext) -> Result<(), String> {
    let relays = PgRelayRepository::new(inbox.db_pool.clone());
    relays
        .update_status(relay_id, RelayStatus::Rejected, None)
        .await
        .map_err(|e| format!("fediverse_relays UPDATE失敗: {}", e))?;
    tracing::info!(
        "[Job::InboundActivityProcess] リレー(id={}) Reject/Undo受信",
        relay_id
    );
    Ok(())
}
