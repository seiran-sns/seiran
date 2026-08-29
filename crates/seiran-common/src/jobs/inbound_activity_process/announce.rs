use super::*;
use super::content::strip_quote_fallback_line_html;
use super::emoji::{has_same_origin, has_unresolved_emoji_shortcodes, record_remote_emojis, resolve_emoji_map_with_fallback};
use super::note_input::{detect_loopback_post_id, extract_ap_quote_uri, guess_attachment_mime_type, normalize_ap_poll, resolve_bridge_duplicate_post_id, strip_quote_fallback_line};


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

    // 公開日時を parse して snowflake ID を生成
    let created_at = published
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap_or_else(|_| chrono::Utc::now());
    let post_id = generate_snowflake_id(created_at);

    // リモートアクターを解決・upsert（未登録なら作成）
    let remote = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let actor_id = remote.actor_id;

    // 元ポストをDBから検索（ap_object_id or at_uri が object_uri と一致するもの）
    let repost_of_post_id = match inbox
        .post_repo
        .find_id_by_ap_or_at_uri(object_uri)
        .await
        .map_err(|e| format!("元ポスト検索失敗: {}", e))?
    {
        Some(id) => id,
        None => {
            tracing::info!(
                "[Inbox/Announce] 元ポストが DB に未存在。リモートからフェッチします: {}",
                object_uri
            );
            match fetch_and_save_note(object_uri, inbox, ap_client).await {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!("[Inbox/Announce] 元ポストの取得・保存に失敗: {}", e);
                    return Ok(());
                }
            }
        }
    };

    // 重複チェック（同一アクターによる同一ポストのリポスト）
    if inbox
        .post_repo
        .find_repost_undo_info(actor_id, repost_of_post_id)
        .await
        .map_err(|e| format!("重複チェック失敗: {}", e))?
        .is_some()
    {
        return Ok(());
    }

    // リポストをDBに挿入
    inbox
        .post_repo
        .insert_repost(
            post_id,
            actor_id,
            announce_id,
            repost_of_post_id,
            created_at,
            visibility,
        )
        .await
        .map_err(|e| format!("リポスト挿入失敗: {}", e))?;

    // リポスト通知: リモート Fedi ユーザーがローカルユーザーの投稿をリポストした場合に作る。
    match inbox.post_repo.find_delivery_meta(repost_of_post_id).await {
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

    tracing::info!(
        "[Inbox/Announce] リポスト保存完了: id={}, actor_id={}, repost_of={}",
        post_id,
        actor_id,
        repost_of_post_id
    );

    Ok(())
}

/// object_uri が指すリモートノートをフェッチして posts テーブルに保存し、その id を返す。
/// 既にレコードが存在する場合は INSERT をスキップして既存 id を返す。
async fn fetch_and_save_note(
    note_uri: &str,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<i64, String> {
    let note = ap_client.fetch_object(note_uri).await?;

    // Note/Question 以外の型（Article 等）は一旦非対応
    if !matches!(note["type"].as_str(), Some("Note") | Some("Question")) {
        return Err(format!(
            "フェッチしたオブジェクトが Note ではありません: type={:?}",
            note["type"]
        ));
    }

    let note_id = note["id"].as_str().unwrap_or(note_uri).to_string();
    let content_html = note["content"].as_str().unwrap_or("").to_string();
    let published = note["published"].as_str().unwrap_or("");

    // attributedTo は文字列または配列どちらもあり得る
    let actor_uri: String = note["attributedTo"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| {
            note["attributedTo"]
                .as_array()?
                .iter()
                .find_map(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .ok_or_else(|| format!("Note ({}) に attributedTo がありません", note_id))?;

    let created_at = published
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap_or_else(|_| chrono::Utc::now());
    let post_id = generate_snowflake_id(created_at);

    // アクターを解決・upsert
    let remote = upsert_remote_fedi_actor(inbox, ap_client, &actor_uri).await?;
    let actor_id = remote.actor_id;

    // `handle_create_note` と同じ絵文字解決ロジック（canonical Note によるtag補完・
    // remote_emojis カタログ記録・フォールバック解決）を適用する。Announce経由で
    // 未知の元ポストをフェッチする経路がこれを行っていなかったため、本文中の
    // カスタム絵文字がショートコードのまま保存される不具合があった（#148）。
    let mut tags = note["tag"].as_array().cloned().unwrap_or_default();
    let mut body = ap_content_to_markdown_body(&content_html, &tags, &remote.domain);
    let mut content_html_sanitized = sanitize_ap_content_html(&content_html, &tags, &remote.domain);
    if has_unresolved_emoji_shortcodes(&tags, &body) && has_same_origin(&note_id, &actor_uri) {
        match ap_client.fetch_object(&note_id).await {
            Ok(canonical_note) => {
                if let Some(canonical_tags) = canonical_note["tag"].as_array() {
                    for tag in canonical_tags {
                        if !tags.contains(tag) {
                            tags.push(tag.clone());
                        }
                    }
                    body = ap_content_to_markdown_body(&content_html, &tags, &remote.domain);
                    content_html_sanitized =
                        sanitize_ap_content_html(&content_html, &tags, &remote.domain);
                }
            }
            Err(error) => {
                tracing::warn!(
                    "[Inbox/Announce] 正規Noteからの絵文字tag補完失敗 note_id={}: {}",
                    note_id,
                    error
                );
            }
        }
    }
    record_remote_emojis(inbox, &remote.domain, &tags).await;
    let emoji_map = resolve_emoji_map_with_fallback(inbox, &remote.domain, &tags, &body).await;

    // 引用URI抽出・解決（#116）。
    let quote_uri = extract_ap_quote_uri(&note, &tags);
    let quote_of_post_id: Option<i64> = match quote_uri.as_deref() {
        Some(uri) => inbox
            .post_repo
            .find_id_by_ap_or_at_uri(uri)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    if let Some(uri) = quote_uri.as_deref() {
        body = strip_quote_fallback_line(&body, uri);
        content_html_sanitized = strip_quote_fallback_line_html(&content_html_sanitized, uri);
    }

    // to/cc から可視性を判定。
    let visibility =
        classify_ap_visibility(&as_string_list(&note["to"]), &as_string_list(&note["cc"]));

    // AP inReplyTo からローカルの reply_to_post_id を解決する。
    let reply_to_post_id: Option<i64> = match note["inReplyTo"].as_str() {
        Some(uri) => inbox
            .post_repo
            .find_id_by_ap_or_at_uri(uri)
            .await
            .ok()
            .flatten(),
        None => None,
    };

    let note_url = note["url"].as_str().unwrap_or("");

    // シナリオ1: ループバックは既存のローカル投稿と同一のため、新規INSERTせずその id を返す。
    if let Some(existing_id) = detect_loopback_post_id(inbox, &note_id, note_url) {
        tracing::warn!(
            "[Inbox/Announce] fetch_and_save_note: ループバック検知、INSERTをスキップ: note_id={} → 既存post_id={}",
            note_id,
            existing_id
        );
        return Ok(existing_id);
    }

    let parent_original_post_id = resolve_bridge_duplicate_post_id(inbox, note_url).await;

    inbox
        .post_repo
        .insert_remote_with_dedup(InsertRemoteWithDedupParams {
            id: post_id,
            actor_id,
            body: &body,
            content_html: Some(&content_html_sanitized),
            ap_object_id: &note_id,
            seiran_uuid: note["seiranUuid"].as_str(),
            parent_original_post_id,
            created_at,
            emoji_map: &emoji_map,
            visibility,
            reply_to_post_id,
            // Announce（リポスト）される投稿がDMであることはAP実装上想定されないため、
            // DMスレッド解決は行わない（direct判定でも thread_root/recipients は空のまま）。
            thread_root_post_id: None,
            recipient_actor_ids: &[],
            quote_of_post_id,
        })
        .await
        .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    let content_warning = note["summary"].as_str().filter(|s| !s.is_empty());
    let poll = normalize_ap_poll(&note);
    inbox
        .post_repo
        .set_fedi_content_metadata(post_id, content_warning, poll.as_ref())
        .await
        .map_err(|e| format!("投稿メタデータ更新エラー: {}", e))?;

    if let Err(e) = inbox.hashtag_repo.link_post(post_id, &body).await {
        tracing::error!(
            "[Inbox/Announce] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}",
            e
        );
    }

    if let Some(attachments) = note["attachment"].as_array() {
        for (position, att) in attachments.iter().enumerate() {
            let url = att["url"]
                .as_str()
                .or_else(|| att.as_str())
                .unwrap_or_default();
            if url.is_empty() {
                continue;
            }
            let mime_type = guess_attachment_mime_type(att, url);
            let is_sensitive = att["sensitive"].as_bool().unwrap_or(false)
                || note["sensitive"].as_bool().unwrap_or(false);
            if let Err(e) = inbox
                .post_repo
                .attach_remote_media_url(
                    post_id,
                    url,
                    mime_type.as_deref(),
                    None,
                    is_sensitive,
                    false,
                    position as i16,
                )
                .await
            {
                tracing::error!("[Inbox/Announce] 添付 URL 保存失敗（スキップ）: {}", e);
            }
        }
    }

    // ON CONFLICT で既存行がある場合も含め、DB 上の id を取得する
    let saved_id = inbox
        .post_repo
        .find_id_by_ap_or_at_uri(&note_id)
        .await
        .map_err(|e| format!("posts id 取得エラー: {}", e))?
        .ok_or_else(|| format!("posts id 取得エラー: {} が見つかりません", note_id))?;

    tracing::info!(
        "[Inbox/Announce] 元ポストをフェッチして保存: id={}, uri={}",
        saved_id,
        note_id
    );
    Ok(saved_id)
}
