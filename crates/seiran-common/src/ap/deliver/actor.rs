use super::activity::*;
use super::infra::*;
use super::*;

/// ローカルアクターの AP Delete(Actor) アクティビティを Fedi フォロワー全員の inbox へ配送する。
/// アカウント退会時（#29）に呼び出し、リモートサーバーにフォロー解除とキャッシュ削除を促す。
pub async fn deliver_delete_actor(
    ap_client: &ApClient,
    db: &PgPool,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;

    let addr = local_actor_address(local_domain, &username);
    let activity_id = format!(
        "https://{}/activities/delete-actor-{}",
        local_domain, actor_id
    );
    let activity =
        build_delete_actor_activity(&addr, &activity_id, &chrono::Utc::now().to_rfc3339());

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!("Delete(Actor) actor_id={} username={}", actor_id, username),
    )
    .await
}

/// ローカルアクターの AP Update(Person) アクティビティを Fedi フォロワー全員の inbox へ配送する。
///
/// プロフィール編集（display_name/bio/avatar）後に呼び出し、リモートインスタンスが
/// キャッシュ済みの Actor 情報をプルせずとも即時更新できるようにする。
pub async fn deliver_update_actor(
    ap_client: &ApClient,
    db: &PgPool,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    ap_public_key_pem: &str,
) -> Result<(), ApError> {
    let row = sqlx::query(
        "SELECT a.username, a.display_name, a.bio, \
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url, \
                mf.mime_type AS avatar_mime_type, a.emoji_map, a.birth_date, a.birth_date_public \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.id = $1 LIMIT 1",
    )
    .bind(actor_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
    .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;

    let username: String = row
        .try_get("username")
        .map_err(|e| ApError::Other(e.to_string()))?;
    let display_name: String = row
        .try_get::<Option<String>, _>("display_name")
        .map_err(|e| ApError::Other(e.to_string()))?
        .unwrap_or_else(|| username.clone());
    let bio: Option<String> = row.try_get("bio").unwrap_or(None);
    let stored_avatar_url: Option<String> = row.try_get("avatar_url").unwrap_or(None);
    let avatar_url = Some(
        stored_avatar_url
            .clone()
            .unwrap_or_else(|| crate::avatar::fallback_avatar_url(local_domain, actor_id)),
    );
    let avatar_mime_type: Option<String> = if stored_avatar_url.is_some() {
        row.try_get("avatar_mime_type").unwrap_or(None)
    } else {
        Some("image/png".to_string())
    };
    let emoji_map: serde_json::Value = row
        .try_get("emoji_map")
        .unwrap_or_else(|_| serde_json::json!({}));
    let birth_date_public: bool = row.try_get("birth_date_public").unwrap_or(false);
    let birth_date = if birth_date_public {
        row.try_get::<Option<chrono::NaiveDate>, _>("birth_date")
            .unwrap_or(None)
    } else {
        None
    };

    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;
    if inboxes.is_empty() {
        return Ok(());
    }

    let addr = local_actor_address(local_domain, &username);
    let person = build_person_object(
        &addr,
        &PersonObjectParams {
            local_domain,
            username: &username,
            display_name: &display_name,
            bio: bio.as_deref(),
            avatar_url: avatar_url.as_deref(),
            avatar_mime_type: avatar_mime_type.as_deref(),
            ap_public_key_pem,
            emoji_map: &emoji_map,
            birth_date,
        },
    );

    // Update は編集の度に配送されうるため、activity id は毎回一意にする
    // （固定IDだと一部実装が2回目以降のUpdateを重複とみなして無視する）。
    let activity_id = format!(
        "https://{}/activities/update-actor-{}-{}",
        local_domain,
        actor_id,
        chrono::Utc::now().timestamp_millis()
    );
    let activity = build_update_actor_activity(
        &addr,
        &activity_id,
        &chrono::Utc::now().to_rfc3339(),
        person,
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!("Update(Actor) actor_id={} username={}", actor_id, username),
    )
    .await
}
