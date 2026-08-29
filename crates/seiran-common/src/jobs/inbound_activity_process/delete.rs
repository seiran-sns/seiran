use super::*;


/// Delete アクティビティを処理し、対象投稿（Note）を論理削除する。
/// `object` は Note の URI（文字列）または `{"type":"Tombstone","id":"..."}` の両形式に対応する。
/// リモートアクター自身の `Delete(Actor)`（退会等）はこの経路では未対応（`object` がどの投稿の
/// `ap_object_id` とも一致しないため、対象なしとして黙って無視される）。
pub(super) async fn handle_delete(activity: serde_json::Value, inbox: &InboxContext) -> Result<(), String> {
    let object = &activity["object"];
    let object_id = match object {
        serde_json::Value::String(s) => Some(s.as_str()),
        serde_json::Value::Object(_) => object["id"].as_str(),
        _ => None,
    };
    let Some(object_id) = object_id else {
        tracing::info!("[Delete] object の id を取得できず無視します");
        return Ok(());
    };

    let Some((post_id, post_actor_id)) = inbox
        .post_repo
        .find_id_and_actor_by_ap_object_id(object_id)
        .await
        .map_err(|e| format!("posts 検索エラー: {}", e))?
    else {
        // 既知の投稿ではない（アクター自身の Delete や、そもそも取り込んでいない投稿等）
        return Ok(());
    };

    // なりすまし対策: Delete の送信元（HTTP Signature 検証済みの actor）が投稿者本人か確認する。
    let actor_uri = activity["actor"].as_str().unwrap_or("");
    let sender = inbox
        .actor_repo
        .find_by_ap_uri(actor_uri)
        .await
        .map_err(|e| format!("送信元アクター検索エラー: {}", e))?;
    if sender.map(|a| a.id) != Some(post_actor_id) {
        tracing::warn!(
            "[Delete] {} の送信元アクター({})が投稿の所有者と一致しないため無視します",
            object_id,
            actor_uri
        );
        return Ok(());
    }

    inbox
        .post_repo
        .soft_delete_by_id(post_id)
        .await
        .map_err(|e| format!("posts (Delete) UPDATE エラー: {}", e))?;
    tracing::info!(
        "[Delete] post_id={} ({}) を削除しました",
        post_id,
        object_id
    );
    Ok(())
}
