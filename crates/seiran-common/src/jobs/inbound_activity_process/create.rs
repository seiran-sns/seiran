use super::*;
use super::content::strip_quote_fallback_line_html;
use super::emoji::{has_same_origin, has_unresolved_emoji_shortcodes, record_remote_emojis, resolve_emoji_map_with_fallback};
use super::note_input::{detect_loopback_post_id, extract_ap_quote_uri, extract_mentioned_local_usernames, guess_attachment_mime_type, normalize_ap_poll, resolve_bridge_duplicate_post_id, strip_quote_fallback_line};
use super::reference::{resolve_reference, RefStatus};


/// 1投稿から抽出するURLカード候補の上限。大量リンクを含む投稿でのOGPフェッチ暴走を防ぐ。
const MAX_LINK_CARDS_PER_POST: usize = 5;

/// 本文（`ap_content_to_markdown_body`が生成したMarkdown）中のリンク`[text](url)`から
/// カード化対象のURLを重複排除しつつ抽出する。画像記法`![...]()`とハッシュタグリンク
/// （表示テキストが`#`始まり）は対象外（メンションはそもそもMarkdownリンクにならない）。
fn extract_link_card_urls(body: &str, max: usize) -> Vec<String> {
    use std::collections::HashSet as Set;
    let re = regex::Regex::new(r"\[([^\]]*)\]\((https?://[^)\s]+)\)").expect("valid regex");
    let mut seen: Set<String> = Set::new();
    let mut urls = Vec::new();
    for cap in re.captures_iter(body) {
        if urls.len() >= max {
            break;
        }
        let full_start = cap.get(0).expect("group 0 always matches").start();
        if full_start > 0 && body.as_bytes().get(full_start - 1) == Some(&b'!') {
            continue;
        }
        let text = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        if text.starts_with('#') {
            continue;
        }
        let url = cap
            .get(2)
            .expect("group 2 always matches")
            .as_str()
            .to_string();
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }
    urls
}

/// 投稿本文中のURLカード化対象URLを、一律OGP取得ジョブ（`Job::OgpFetch`、OGPタグに加えて
/// oEmbed discoveryによる埋め込みプレーヤー解決も行う）へ積む。投稿保存自体は既に完了して
/// いるため、ここでの失敗はログのみでハンドラ全体を失敗させない。
async fn queue_link_cards_for_post(queue: &Arc<dyn JobQueue>, post_id: i64, body: &str) {
    let urls = extract_link_card_urls(body, MAX_LINK_CARDS_PER_POST);
    for (position, url) in urls.into_iter().enumerate() {
        let position = position as i16;
        if let Err(e) = queue
            .enqueue(
                Job::OgpFetch {
                    post_id,
                    url,
                    position,
                },
                priority::LOW,
            )
            .await
        {
            tracing::error!("[Create/Note] OgpFetch enqueue失敗: {}", e);
        }
    }
}

/// リモート投稿に起因するローカルユーザーへの通知（引用・リプライ・メンション）を、
/// リアルタイムイベント配信と通知レコード挿入の対で作る。3 種で完全に同形のため共通化する
/// （how: 通知の生成・配信）。呼び出し側は「誰に・どの種別で」だけを決める（what）。
pub(super) async fn notify_local_actor(
    inbox: &InboxContext,
    target_actor_id: i64,
    kind: NotificationKind,
    event_name: &str,
    from_actor_id: i64,
    post_id: i64,
    remote: &RemoteActorInfo,
) {
    inbox.stream_hub.publish_event(
        HashSet::from([target_actor_id]),
        event_name,
        serde_json::json!({
            "postId": post_id.to_string(),
            "actor": { "username": remote.username, "domain": remote.domain, "displayName": remote.display_name },
        }),
    );
    let notif_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = inbox
        .notification_repo
        .insert(
            notif_id,
            target_actor_id,
            kind,
            Some(from_actor_id),
            Some(post_id),
            None,
            None,
            None,
            None,
            None,
        )
        .await
    {
        tracing::error!("[Create/Note] {} notification INSERT 失敗: {}", event_name, e);
    }
}

/// AP Note の `attachment` 配列を、投稿の添付メディア URL として保存する
/// （S3 には保存せず URL のみ記録。how: 添付の永続化）。
pub(super) async fn save_remote_attachments(
    inbox: &InboxContext,
    post_id: i64,
    note: &serde_json::Value,
) {
    let Some(attachments) = note["attachment"].as_array() else {
        return;
    };
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
            tracing::error!("[Create/Note] 添付 URL 保存失敗（スキップ）: {}", e);
        }
    }
}

/// `broadcast_created_note` の入力。保存済みリモート投稿を WebSocket 配信するのに必要な値だけを束ねる。
pub(super) struct CreatedNoteContext<'a> {
    pub post_id: i64,
    pub body: &'a str,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub actor_id: i64,
    pub remote: &'a RemoteActorInfo,
    pub emoji_map: &'a serde_json::Value,
    pub visibility: &'a str,
    pub reply_to_post_id: Option<i64>,
    pub recipient_actor_ids: &'a [i64],
}

/// 保存済みリモート投稿を可視性に応じて WebSocket 配信する（how: リアルタイム配信）。
/// direct は宛先のみ（本文漏洩防止のためフォロワーには配信しない）、それ以外は
/// タイムライン系チャンネル（home/local/hybrid/global/userList/hashtag）購読者へ流す。
pub(super) async fn broadcast_created_note(inbox: &InboxContext, ctx: CreatedNoteContext<'_>) {
    let note_json = serde_json::json!({
        "id": ctx.post_id.to_string(),
        "text": ctx.body,
        "createdAt": ctx.created_at.to_rfc3339(),
        "user": {
            "id": ctx.actor_id,
            "username": ctx.remote.username,
            "domain": ctx.remote.domain,
            "displayName": ctx.remote.display_name,
            "actorType": "fedi",
            "avatarUrl": ctx.remote.avatar_url,
        },
        "attachments": [],
        "emojis": ctx.emoji_map,
    });
    if ctx.visibility == "direct" {
        let recipients: HashSet<i64> = ctx.recipient_actor_ids.iter().copied().collect();
        if !recipients.is_empty() {
            let mut note_json = note_json;
            note_json["visibility"] = serde_json::json!("direct");
            inbox.stream_hub.publish_note(recipients, &note_json);
        }
    } else {
        let mut home_recipients: HashSet<i64> = inbox
            .follow_repo
            .find_home_recipient_ids(ctx.actor_id, ctx.reply_to_post_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        home_recipients.insert(ctx.actor_id);
        let list_ids: HashSet<i64> = inbox
            .list_repo
            .list_ids_containing_actor(ctx.actor_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        let hashtags: HashSet<String> = crate::hashtag::extract_hashtags(ctx.body)
            .into_iter()
            .collect();
        let scope = ChannelScope {
            is_local: false,
            visibility: ctx.visibility.to_string(),
            home_recipients: Arc::new(home_recipients),
            list_ids: Arc::new(list_ids),
            hashtags: Arc::new(hashtags),
        };
        inbox.stream_hub.publish_channel_note(scope, note_json);
    }
}

// Create(Note) を受け取り posts テーブルに保存する
pub(super) async fn handle_create_note(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
    queue: &Arc<dyn JobQueue>,
) -> Result<(), String> {
    let note = &activity["object"];
    let note_id = note["id"].as_str().ok_or("Note: id がありません")?;

    // 同一 Create/Note の再配送では投稿本体だけでなく、引用・返信・メンション通知も
    // 二重生成しない。insert_remote_with_dedup の ON CONFLICT より前に終了する。
    if inbox
        .post_repo
        .find_id_by_ap_or_at_uri(note_id)
        .await
        .map_err(|e| format!("Create/Note 重複チェック失敗: {}", e))?
        .is_some()
    {
        return Ok(());
    }

    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("Create: actor がありません")?;
    let content_html = note["content"].as_str().unwrap_or("").to_string();
    let published = note["published"].as_str().unwrap_or("");

    // 公開日時を parse して snowflake ID を生成
    let created_at = published
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap_or_else(|_| chrono::Utc::now());
    let post_id = generate_snowflake_id(created_at);

    // リモートアクターを解決・upsert（未登録なら作成）
    let remote = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let actor_id = remote.actor_id;

    // HTML タグを除去して本文を得る（<a href> はリンクとして保持し、Markdownリンク記法
    // `[text](url)` に変換する。メンションは `@user@host` のプレーンテキストに正規化）。
    let mut tags = note["tag"].as_array().cloned().unwrap_or_default();
    let mut body = ap_content_to_markdown_body(&content_html, &tags, &remote.domain);
    // seiran Web UI でのリッチ表示用（`<blockquote>`/`<ruby>`等の構造保持、#233）。
    // `body`とは別に、意味的構造をクレンジングして保持したHTMLを`content_html`列に持つ。
    let mut content_html_sanitized = sanitize_ap_content_html(&content_html, &tags, &remote.domain);
    // リレー実装によっては、配送する Create の埋め込み Note から Emoji tag を
    // 省略する一方、object.id の正規 Note には完全な tag を載せる。本文に未解決の
    // shortcode がある場合だけ正規 Note を取得し、欠落した tag を補完する。
    // object.id は外部入力なので、解決済み投稿者actorと同一originの場合だけ取得する。
    if has_unresolved_emoji_shortcodes(&tags, &body) && has_same_origin(note_id, actor_uri) {
        match ap_client.fetch_object(note_id).await {
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
                    "[Create/Note] 正規Noteからの絵文字tag補完失敗 note_id={}: {}",
                    note_id,
                    error
                );
            }
        }
    }
    // 本文中のカスタム絵文字（`:shortcode:`）→画像URLマップ（AP Note の tag 配列由来）。
    record_remote_emojis(inbox, &remote.domain, &tags).await;
    let emoji_map = resolve_emoji_map_with_fallback(inbox, &remote.domain, &tags, &body).await;

    // 引用URI抽出・解決（#116）。DBに無ければ1段階だけフェッチを試みる（#231）。
    // 取得できた場合、Misskey/Fedibirdが本文末尾に自動付加する
    // `RE:`/`QT:` フォールバック行（引用URIと同じURLを指す）を本文から取り除く。
    let quote_uri = extract_ap_quote_uri(note, &tags);
    let (quote_of_post_id, quote_of_ap_uri, quote_of_ref_status) =
        resolve_reference(quote_uri.as_deref(), inbox, ap_client)
            .await
            .into_parts();
    if let Some(uri) = quote_uri.as_deref() {
        body = strip_quote_fallback_line(&body, uri);
        content_html_sanitized = strip_quote_fallback_line_html(&content_html_sanitized, uri);
    }
    // to/cc から可視性を判定（#配送先・可視性アイコン追加）。
    let to_list = as_string_list(&note["to"]);
    let visibility = classify_ap_visibility(&to_list, &as_string_list(&note["cc"]));

    // AP inReplyTo からローカルの reply_to_post_id を解決する（DM機能実装以前はこの解決自体が
    // 存在しなかった。通常投稿にも有用だが、direct（DM）のスレッド起点伝播に必須のため追加）。
    // DBに無ければ1段階だけフェッチを試みる（#231）。
    let (reply_to_post_id, reply_to_ap_uri, reply_to_ref_status) =
        resolve_reference(note["inReplyTo"].as_str(), inbox, ap_client)
            .await
            .into_parts();

    // リプライ先投稿者のactor_id（ローカルユーザーの場合のみ）。リプライ通知の宛先に使う。
    let reply_parent_local_actor_id: Option<i64> = match reply_to_post_id {
        Some(parent_id) => inbox
            .post_repo
            .find_delivery_meta(parent_id)
            .await
            .ok()
            .flatten()
            .filter(|m| m.actor_type == "local")
            .map(|m| m.actor_id),
        None => None,
    };

    // DM（visibility="direct"）の宛先・スレッド起点解決。
    // `to`に含まれるローカルアクターURI（`https://{local_domain}/users/{username}`）を宛先とする。
    let (thread_root_post_id, recipient_actor_ids): (Option<i64>, Vec<i64>) = if visibility
        == "direct"
    {
        let parent_thread_root = match reply_to_post_id {
            Some(parent_id) => inbox
                .post_repo
                .find_delivery_meta(parent_id)
                .await
                .ok()
                .flatten()
                .and_then(|m| {
                    if m.visibility == "direct" {
                        m.thread_root_post_id
                    } else {
                        None
                    }
                }),
            None => None,
        };
        let thread_root = parent_thread_root.unwrap_or(post_id);

        // ローカルユーザーの `actors.ap_uri` は登録時に設定されない（都度
        // `https://{local_domain}/users/{username}` として動的組み立てされる）ため
        // `find_by_ap_uri` では引っかからない。`extract_local_username` で
        // ホスト名まで含めて自ドメインのURIか検証してから解決する（末尾セグメント
        // だけを見ると、リモートの同名ユーザー宛のDMをローカルの同名ユーザー宛だと
        // 誤認してしまう）。
        let mut recipients = Vec::new();
        for uri in &to_list {
            let Some(local_username) = crate::ap::extract_local_username(uri, &inbox.local_domain)
            else {
                continue;
            };
            if let Ok(Some(actor)) = inbox
                .actor_repo
                .find_by_username_domain(local_username, &inbox.local_domain)
                .await
            {
                if actor.actor_type == "local" {
                    recipients.push(actor.id);
                }
            }
        }
        (Some(thread_root), recipients)
    } else {
        (None, Vec::new())
    };

    // シナリオ2: seiran_post_uuid による seiran 間マージ
    let seiran_uuid = note["seiranUuid"].as_str();
    if let Some(uuid) = seiran_uuid {
        if let Some((existing_id, existing_ap_id)) = inbox
            .post_repo
            .find_by_seiran_uuid(uuid)
            .await
            .map_err(|e| format!("seiran_post_uuid 検索失敗: {}", e))?
        {
            if existing_ap_id.is_none() {
                // ap_object_id 未設定なら UPDATE
                inbox
                    .post_repo
                    .update_ap_object_id(existing_id, note_id)
                    .await
                    .map_err(|e| format!("ap_object_id 更新失敗: {}", e))?;
                tracing::info!(
                    "[Create/Note] seiran_uuid マージ（AP 側更新）: id={}",
                    existing_id
                );
            }
            // 重複インサートはしない
            return Ok(());
        }
    }

    let note_url = note["url"].as_str().unwrap_or("");

    // シナリオ1: ループバックは既存のローカル投稿の重複でしかないため、新規INSERTせず無視する。
    if let Some(existing_id) = detect_loopback_post_id(inbox, note_id, note_url) {
        tracing::warn!(
            "[Create/Note] ループバック検知、INSERTをスキップ: note_id={} → 既存post_id={}",
            note_id,
            existing_id
        );
        return Ok(());
    }

    let parent_original_post_id = resolve_bridge_duplicate_post_id(inbox, note_url).await;

    // posts テーブルに挿入（ap_object_id 重複はスキップ、seiran_post_uuid も保存）
    inbox
        .post_repo
        .insert_remote_with_dedup(InsertRemoteWithDedupParams {
            id: post_id,
            actor_id,
            body: &body,
            content_html: Some(&content_html_sanitized),
            ap_object_id: note_id,
            seiran_uuid,
            parent_original_post_id,
            created_at,
            emoji_map: &emoji_map,
            visibility,
            reply_to_post_id,
            reply_to_ap_uri: reply_to_ap_uri.as_deref(),
            reply_to_ref_status: reply_to_ref_status.map(RefStatus::as_db_str),
            thread_root_post_id,
            recipient_actor_ids: &recipient_actor_ids,
            quote_of_post_id,
            quote_of_ap_uri: quote_of_ap_uri.as_deref(),
            quote_of_ref_status: quote_of_ref_status.map(RefStatus::as_db_str),
        })
        .await
        .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    let content_warning = note["summary"].as_str().filter(|s| !s.is_empty());
    let poll = normalize_ap_poll(note);
    inbox
        .post_repo
        .set_fedi_content_metadata(post_id, content_warning, poll.as_ref())
        .await
        .map_err(|e| format!("投稿メタデータ更新エラー: {}", e))?;

    if let Err(e) = inbox.hashtag_repo.link_post(post_id, &body).await {
        tracing::error!(
            "[Create/Note] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}",
            e
        );
    }

    // URLカード（OGP取得ジョブがoEmbed discoveryによる埋め込みプレーヤー解決も行う）。
    queue_link_cards_for_post(queue, post_id, &body).await;

    // 引用通知: リモート Fedi ユーザーがローカルユーザーの投稿を引用した場合に作る。
    if let Some(quoted_post_id) = quote_of_post_id {
        match inbox.post_repo.find_delivery_meta(quoted_post_id).await {
            Ok(Some(meta)) if meta.actor_type == "local" && meta.actor_id != actor_id => {
                notify_local_actor(
                    inbox,
                    meta.actor_id,
                    NotificationKind::Quote,
                    "quote",
                    actor_id,
                    post_id,
                    &remote,
                )
                .await;
            }
            Ok(_) => {}
            Err(e) => tracing::error!("[Create/Note] 引用元メタ情報の取得に失敗: {}", e),
        }
    }

    // リプライ通知: リプライ先がローカルユーザーの投稿であれば通知を作る（自己リプライは除く）。
    if let Some(parent_actor_id) = reply_parent_local_actor_id.filter(|id| *id != actor_id) {
        notify_local_actor(
            inbox,
            parent_actor_id,
            NotificationKind::Reply,
            "reply",
            actor_id,
            post_id,
            &remote,
        )
        .await;
    }

    // メンション通知: `tag[]` の `Mention` がローカルユーザーの AP actor URI
    // （`https://{local_domain}/users/{username}`）を指す場合、通知を作る。
    // ローカルユーザーの `ap_uri` は動的組み立てのため、DM宛先解決（上記）と同じ
    // `extract_local_username` でホスト名まで検証してから解決する（他インスタンスの
    // 同名ユーザーを誤って拾わないため。詳細は下のテスト参照）。
    let mut mentioned_local_actor_ids: Vec<i64> = Vec::new();
    for local_username in extract_mentioned_local_usernames(&tags, &inbox.local_domain) {
        if let Ok(Some(actor)) = inbox
            .actor_repo
            .find_by_username_domain(local_username, &inbox.local_domain)
            .await
        {
            if actor.actor_type == "local" && !mentioned_local_actor_ids.contains(&actor.id) {
                mentioned_local_actor_ids.push(actor.id);
            }
        }
    }
    for mentioned_actor_id in mentioned_local_actor_ids {
        notify_local_actor(
            inbox,
            mentioned_actor_id,
            NotificationKind::Mention,
            "mention",
            actor_id,
            post_id,
            &remote,
        )
        .await;
    }

    // 添付画像・動画・音声の URL を保存（S3 には保存せず URL のみ記録）
    save_remote_attachments(inbox, post_id, note).await;

    // WebSocket リアルタイム配信。
    broadcast_created_note(
        inbox,
        CreatedNoteContext {
            post_id,
            body: &body,
            created_at,
            actor_id,
            remote: &remote,
            emoji_map: &emoji_map,
            visibility,
            reply_to_post_id,
            recipient_actor_ids: &recipient_actor_ids,
        },
    )
    .await;

    let dup_info =
        parent_original_post_id.map_or(String::new(), |id| format!(" (parent_original={})", id));
    tracing::info!(
        "[Create/Note] {} から投稿を受信・保存: {}{}",
        actor_uri,
        note_id,
        dup_info
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_link_card_urls_finds_multiple_and_dedups() {
        let body = "見て [記事](https://example.com/a) それと [同じ記事](https://example.com/a) [別記事](https://example.com/b)";
        let urls = extract_link_card_urls(body, 5);
        assert_eq!(
            urls,
            vec![
                "https://example.com/a".to_string(),
                "https://example.com/b".to_string()
            ]
        );
    }

    #[test]
    fn extract_link_card_urls_ignores_hashtag_links() {
        let body = "[#foo](https://example.social/tags/foo) [記事](https://example.com/a)";
        let urls = extract_link_card_urls(body, 5);
        assert_eq!(urls, vec!["https://example.com/a".to_string()]);
    }

    #[test]
    fn extract_link_card_urls_ignores_image_markdown() {
        let body = "![alt](https://example.com/pic.png) [記事](https://example.com/a)";
        let urls = extract_link_card_urls(body, 5);
        assert_eq!(urls, vec!["https://example.com/a".to_string()]);
    }

    #[test]
    fn extract_link_card_urls_respects_max() {
        let body =
            "[a](https://example.com/1) [b](https://example.com/2) [c](https://example.com/3)";
        let urls = extract_link_card_urls(body, 2);
        assert_eq!(urls.len(), 2);
    }
}
