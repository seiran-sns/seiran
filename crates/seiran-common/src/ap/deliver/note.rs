use super::activity::*;
use super::infra::*;
use super::note_object::build_local_note;
use super::ApSender;
use super::*;

// =====================================================================
// 配送オーケストレーション（公開 API）
// =====================================================================

/// ローカル投稿を AP フォロワー全員の inbox へ Create(Note) で配送する。
/// Note は`note_object::build_local_note`が DB から組み立てる（`GET /notes/:id`と同じもの）。
pub async fn deliver_post_to_ap_followers(
    sender: &ApSender<'_>,
    post_id: i64,
) -> Result<(), ApError> {
    let ApSender {
        ap_client,
        db,
        actor_id,
        local_domain,
        private_key_pem: ap_private_key_pem,
    } = *sender;
    let Some(note) = build_local_note(ap_client, db, local_domain, post_id, actor_id).await? else {
        tracing::warn!(
            "[deliver_post_to_ap_followers] Note として配送できない投稿のためスキップ（post_id={}）",
            post_id
        );
        return Ok(());
    };

    // DM（direct）はこの関数（フォロワー全体へのファンアウト）では扱わない。
    // `deliver_direct_message_to_ap` を使うこと（呼び出し元の実装ミスに対する最終ガード）。
    if note.visibility == "direct" {
        tracing::warn!("[deliver_post_to_ap_followers] visibility=direct のポストが渡されたためスキップ（post_id={}）", post_id);
        return Ok(());
    }

    // 配送先はフォロワー + 本文中でメンションした相手（フォロワーでなくても通知を届ける）の和集合。
    let mut inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;
    for inbox in fetch_inboxes_by_ap_uris(
        ap_client,
        db,
        local_domain,
        ap_private_key_pem,
        &note.mention_uris,
    )
    .await
    {
        if !inboxes.contains(&inbox) {
            inboxes.push(inbox);
        }
    }
    // リプライ/引用の場合は対象ポストを巡る会話の参加者にも配送を広げる
    // （`resolve_conversation_broadcast_inboxes` 参照、#235）。
    let refs = fetch_post_reference_ids(db, post_id).await?;
    for target_post_id in [refs.reply_to_post_id, refs.quote_of_post_id]
        .into_iter()
        .flatten()
    {
        for inbox in resolve_conversation_broadcast_inboxes(db, target_post_id).await? {
            if !inboxes.contains(&inbox) {
                inboxes.push(inbox);
            }
        }
    }
    // public投稿のみ、参加中のリレー（#140）にもファンアウトする。
    // unlisted/followers_only はリレー配送対象外（リレーは公開投稿の中継が目的のため）。
    if note.visibility == "public" {
        for inbox in fetch_accepted_relay_inboxes(db).await? {
            if !inboxes.contains(&inbox) {
                inboxes.push(inbox);
            }
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let addr = local_actor_address(local_domain, &note.username);
    let activity = build_create_note_activity(&addr, local_domain, post_id, &note.object);

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Create(Note) post_id={} username={}",
            post_id, note.username
        ),
    )
    .await
}

/// ATPコミット完了（`posts.at_uri`確定）後に、`seiranPost.counterpartPostId`を
/// 補完した`Update(Note)`をAPフォロワーへ送る（#237、配送側の制約「非対称・後からUpdateで補完」）。
/// Create(Note)を送っていない投稿（`deliver_fedi=false`等）や、ATP DIDを持たない投稿者
/// （シングルホストモード）の場合は送るものが無いため何もしない。
pub async fn deliver_seiranpost_update(sender: &ApSender<'_>, post_id: i64) -> Result<(), ApError> {
    let ApSender {
        ap_client,
        db,
        actor_id,
        local_domain,
        private_key_pem: ap_private_key_pem,
    } = *sender;
    let Some(note) = build_local_note(ap_client, db, local_domain, post_id, actor_id).await? else {
        return Ok(());
    };
    if note.visibility == "direct" || note.object.get("seiranPost").is_none() {
        return Ok(());
    }
    if !note.has_counterpart {
        // ATPコミットがまだ確定していない（呼び出し元のタイミングミス）。送るものが無い。
        tracing::warn!(
            "[deliver_seiranpost_update] at_uri未確定のままUpdate(Note)呼び出し post_id={}",
            post_id
        );
        return Ok(());
    }

    let mut inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;
    for inbox in fetch_inboxes_by_ap_uris(
        ap_client,
        db,
        local_domain,
        ap_private_key_pem,
        &note.mention_uris,
    )
    .await
    {
        if !inboxes.contains(&inbox) {
            inboxes.push(inbox);
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let addr = local_actor_address(local_domain, &note.username);
    let now = chrono::Utc::now().to_rfc3339();
    let activity = build_update_note_activity(&addr, local_domain, post_id, &note.object, &now);

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Update(Note seiranPost) post_id={} username={}",
            post_id, note.username
        ),
    )
    .await
}

/// DM（`visibility='direct'`）投稿を、宛先（`post_recipients`）の中のFediアクターへのみ
/// 配送する。`deliver_post_to_ap_followers`（フォロワー全体へのファンアウト）とは異なり、
/// フォロワーコレクションではなく実際の宛先個人のinboxのみへCreate(Note)を送る。
pub async fn deliver_direct_message_to_ap(
    sender: &ApSender<'_>,
    post_id: i64,
) -> Result<(), ApError> {
    let ApSender {
        ap_client,
        db,
        actor_id,
        local_domain,
        private_key_pem: ap_private_key_pem,
    } = *sender;

    let inbox_rows = sqlx::query(
        "SELECT a.ap_inbox_url
         FROM post_recipients pr JOIN actors a ON a.id = pr.actor_id
         WHERE pr.post_id = $1 AND a.actor_type IN ('fedi', 'remote_seiran') AND a.ap_uri IS NOT NULL AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("DM宛先取得エラー: {}", e)))?;
    let inboxes: Vec<String> = inbox_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_inbox_url").ok())
        .collect();
    if inboxes.is_empty() {
        return Ok(());
    }

    let Some(note) = build_local_note(ap_client, db, local_domain, post_id, actor_id).await? else {
        return Ok(());
    };
    if note.visibility != "direct" {
        tracing::warn!(
            "[deliver_direct_message_to_ap] visibility={} のポストが渡されたためスキップ（post_id={}）",
            note.visibility, post_id
        );
        return Ok(());
    }

    let addr = local_actor_address(local_domain, &note.username);
    let activity = build_create_note_activity(&addr, local_domain, post_id, &note.object);

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Create(Note DM) post_id={} username={}",
            post_id, note.username
        ),
    )
    .await
}

/// 投稿の添付ファイル群を AP Document オブジェクトのリストとして取得する。
pub(super) async fn fetch_attachment_documents(
    db: &PgPool,
    post_id: i64,
) -> Result<Vec<serde_json::Value>, ApError> {
    let rows = sqlx::query(
        "SELECT mf.storage_key, mf.mime_type, mf.width, mf.height, mf.blurhash, sp.public_url,
                pa.is_sensitive
         FROM post_attachments pa
         JOIN media_files mf ON mf.id = pa.media_file_id
         JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE pa.post_id = $1
         ORDER BY pa.position",
    )
    .bind(post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("添付取得エラー: {}", e)))?;

    Ok(rows
        .iter()
        .filter_map(|r| {
            let storage_key: String = r.try_get("storage_key").ok()?;
            let mime_type: String = r.try_get("mime_type").ok()?;
            let width: Option<i32> = r.try_get("width").ok()?;
            let height: Option<i32> = r.try_get("height").ok()?;
            let blurhash: Option<String> = r.try_get("blurhash").ok()?;
            let public_url: String = r.try_get("public_url").ok()?;
            let sensitive: bool = r.try_get("is_sensitive").ok()?;
            Some(build_attachment_document(&AttachmentDocument {
                public_url: &public_url,
                storage_key: &storage_key,
                mime_type: &mime_type,
                width,
                height,
                blurhash: blurhash.as_deref(),
                sensitive,
            }))
        })
        .collect())
}

/// ローカルアクターの AP Delete(Note) アクティビティを Fedi フォロワー全員の inbox へ配送する。
/// `post_id` はリポスト投稿の posts.id（`PostToFollowers` で送った Note の id
/// `https://{domain}/notes/{post_id}` と一致する）。
pub async fn deliver_delete_note(sender: &ApSender<'_>, post_id: i64) -> Result<(), ApError> {
    let ApSender {
        ap_client,
        db,
        actor_id,
        local_domain,
        private_key_pem: ap_private_key_pem,
    } = *sender;
    let username = fetch_username(db, actor_id).await?;
    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;

    let addr = local_actor_address(local_domain, &username);
    let note_id = format!("https://{}/notes/{}", local_domain, post_id);
    let activity_id = format!(
        "https://{}/activities/delete-note-{}",
        local_domain, post_id
    );
    let activity = build_delete_note_activity(
        &addr,
        &note_id,
        &activity_id,
        &chrono::Utc::now().to_rfc3339(),
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!("Delete(Note) post_id={} username={}", post_id, username),
    )
    .await
}
