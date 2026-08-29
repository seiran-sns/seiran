use super::*;
use super::infra::*;
use super::activity::*;


// =====================================================================
// 配送オーケストレーション（公開 API）
// =====================================================================

/// ローカル投稿を AP フォロワー全員の inbox へ配送する
///
/// `override_body` が `Some` の場合はその値を本文として使用する（AP向けメンション変換済みテキスト等）。
/// `None` の場合は DB の `posts.body` をそのまま使用する。
/// `quote_url` が `Some` の場合は Note に `quoteUrl` / `_misskey_quote` を付与する（引用投稿）。
/// seiran_post_uuid は DB の posts.seiran_post_uuid から自動取得して Note に付与する。
#[allow(clippy::too_many_arguments)]
pub async fn deliver_post_to_ap_followers(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    override_body: Option<&str>,
    quote_url: Option<&str>,
    in_reply_to: Option<&str>,
) -> Result<(), ApError> {
    let basis = fetch_post_activity_basis(db, post_id, actor_id).await?;

    // DM（direct）はこの関数（フォロワー全体へのファンアウト）では扱わない。
    // `deliver_direct_message_to_ap` を使うこと（呼び出し元の実装ミスに対する最終ガード）。
    if basis.visibility == "direct" {
        tracing::warn!("[deliver_post_to_ap_followers] visibility=direct のポストが渡されたためスキップ（post_id={}）", post_id);
        return Ok(());
    }

    let body: String = override_body.map(str::to_owned).unwrap_or(basis.body);

    // override_body（リポストのフォールバックテキスト等、投稿者本人が書いた本文ではない合成テキスト）
    // の場合はメンション変換をせずそのまま HTML 化する。通常投稿（override_body なし）はここで
    // 本文中のメンションを解決し、`<a>` アンカーと `tag[]`（AP Mention）を組み立てる。
    let (content_html, mut tag, mention_uris): (String, Vec<serde_json::Value>, Vec<String>) =
        if override_body.is_some() {
            (plain_to_html(&body), Vec::new(), Vec::new())
        } else {
            html_and_tags_for_body(&body, local_domain, db, ap_client).await
        };
    append_emoji_tags(&body, &basis.emoji_map, &mut tag, local_domain);

    // 配送先はフォロワー + 本文中でメンションした相手（フォロワーでなくても通知を届ける）の和集合。
    let mut inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;
    for inbox in fetch_inboxes_by_ap_uris(ap_client, db, local_domain, &mention_uris).await {
        if !inboxes.contains(&inbox) {
            inboxes.push(inbox);
        }
    }
    // public投稿のみ、参加中のリレー（#140）にもファンアウトする。
    // unlisted/followers_only はリレー配送対象外（リレーは公開投稿の中継が目的のため）。
    if basis.visibility == "public" {
        for inbox in fetch_accepted_relay_inboxes(db).await? {
            if !inboxes.contains(&inbox) {
                inboxes.push(inbox);
            }
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let addr = local_actor_address(local_domain, &basis.username);
    let activity = build_create_note_activity(
        &addr,
        &NoteActivityParams {
            local_domain,
            post_id,
            content_html: &content_html,
            published: &basis.created_at.to_rfc3339(),
            attachments: basis.attachments,
            quote_url,
            in_reply_to,
            seiran_uuid: basis.seiran_uuid.as_deref(),
            visibility: &basis.visibility,
            tag,
            direct_recipients: &[],
            mention_recipients: &mention_uris,
            poll: basis.poll.as_ref(),
            content_warning: basis.content_warning.as_deref(),
        },
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Create(Note) post_id={} username={}",
            post_id, basis.username
        ),
    )
    .await
}

/// DM（`visibility='direct'`）投稿を、宛先（`post_recipients`）の中のFediアクターへのみ
/// 配送する。`deliver_post_to_ap_followers`（フォロワー全体へのファンアウト）とは異なり、
/// フォロワーコレクションではなく実際の宛先個人のinboxのみへCreate(Note)を送る。
pub async fn deliver_direct_message_to_ap(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), ApError> {
    let basis = fetch_post_activity_basis(db, post_id, actor_id).await?;

    let recipient_rows = sqlx::query(
        "SELECT a.ap_uri, a.ap_inbox_url
         FROM post_recipients pr JOIN actors a ON a.id = pr.actor_id
         WHERE pr.post_id = $1 AND a.actor_type = 'fedi' AND a.ap_uri IS NOT NULL AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("DM宛先取得エラー: {}", e)))?;

    if recipient_rows.is_empty() {
        return Ok(());
    }

    let direct_recipients: Vec<String> = recipient_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_uri").ok())
        .collect();
    let inboxes: Vec<String> = recipient_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_inbox_url").ok())
        .collect();

    let (content_html, mut tag, _mention_uris) =
        html_and_tags_for_body(&basis.body, local_domain, db, ap_client).await;
    append_emoji_tags(&basis.body, &basis.emoji_map, &mut tag, local_domain);

    let addr = local_actor_address(local_domain, &basis.username);
    let activity = build_create_note_activity(
        &addr,
        &NoteActivityParams {
            local_domain,
            post_id,
            content_html: &content_html,
            published: &basis.created_at.to_rfc3339(),
            attachments: basis.attachments,
            quote_url: None,
            in_reply_to: None,
            seiran_uuid: None,
            visibility: "direct",
            tag,
            direct_recipients: &direct_recipients,
            // directは`direct_recipients`が既に実際の宛先そのものなので無視される（visibility_to_to_cc参照）。
            mention_recipients: &[],
            // DMではアンケート作成自体が禁止されているため常にNone（`notes/mod.rs`の
            // `POLL_NOT_ALLOWED_FOR_DM`参照）。
            poll: None,
            // DMではCW作成自体が禁止されているため常にNone（`CW_NOT_ALLOWED_FOR_DM`参照）。
            content_warning: None,
        },
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Create(Note DM) post_id={} username={}",
            post_id, basis.username
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
        "SELECT mf.storage_key, mf.mime_type, mf.width, mf.height, mf.blurhash, sp.public_url
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
            Some(build_attachment_document(
                &public_url,
                &storage_key,
                &mime_type,
                width,
                height,
                blurhash.as_deref(),
            ))
        })
        .collect())
}

/// ローカルアクターの AP Delete(Note) アクティビティを Fedi フォロワー全員の inbox へ配送する。
/// `post_id` はリポスト投稿の posts.id（`PostToFollowers` で送った Note の id
/// `https://{domain}/notes/{post_id}` と一致する）。
pub async fn deliver_delete_note(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), ApError> {
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
