use super::content::strip_quote_fallback_line_html;
use super::emoji::{
    has_same_origin, has_unresolved_emoji_shortcodes, record_remote_emojis,
    resolve_emoji_map_with_fallback,
};
use super::note_input::{
    detect_loopback_post_id, extract_ap_quote_uri, guess_attachment_mime_type, normalize_ap_poll,
    resolve_bridge_duplicate_post_id, strip_quote_fallback_line,
};
use super::*;

/// リプライ/引用/リポストの参照先が未解決（`pending`）か消失確認済み（`gone`）かを表す（#230）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefStatus {
    Pending,
    Gone,
}

impl RefStatus {
    pub fn as_db_str(self) -> &'static str {
        match self {
            RefStatus::Pending => "pending",
            RefStatus::Gone => "gone",
        }
    }
}

/// リプライ/引用/リポストの参照解決結果。
pub enum ReferenceOutcome {
    /// 参照が無い（`inReplyTo`/`quoteUrl`/repost対象がそもそも無い）。
    None,
    /// ローカルDBの`posts.id`まで解決できた。
    Resolved(i64),
    /// 未解決。生のAP URIと状態（pending/gone）を保持する。
    Unresolved { ap_uri: String, status: RefStatus },
}

impl ReferenceOutcome {
    /// `InsertRemoteWithDedupParams`等へそのまま渡せる (post_id, ap_uri, ref_status) の3つ組に分解する。
    pub fn into_parts(self) -> (Option<i64>, Option<String>, Option<RefStatus>) {
        match self {
            ReferenceOutcome::None => (None, None, None),
            ReferenceOutcome::Resolved(id) => (Some(id), None, None),
            ReferenceOutcome::Unresolved { ap_uri, status } => (None, Some(ap_uri), Some(status)),
        }
    }
}

/// DB照合のみで参照を解決する（フェッチしない）。`save_fetched_remote_note`が保存する
/// ノート自身の参照（リプライ元・引用元）はこちらを使う。これにより「1段階だけフェッチする」
/// （トップレベルの`resolve_reference`だけがフェッチし、その先は辿らない）という制約を守る。
pub(super) async fn resolve_reference_db_only(
    uri: Option<&str>,
    inbox: &InboxContext,
) -> ReferenceOutcome {
    let Some(uri) = uri else {
        return ReferenceOutcome::None;
    };
    match inbox.post_repo.find_id_by_ap_or_at_uri(uri).await {
        Ok(Some(id)) => ReferenceOutcome::Resolved(id),
        _ => ReferenceOutcome::Unresolved {
            ap_uri: uri.to_string(),
            status: RefStatus::Pending,
        },
    }
}

/// `ap_client.fetch_object`に渡すシステムアクター（list-relay）の署名鍵（キーID, 秘密鍵PEM）
/// を組み立てる。Authorized Fetch（secure mode）を要求するリモートでも参照解決できるよう、
/// 1段階フェッチは常にこの鍵で署名する。
pub(super) fn system_signing_key(inbox: &InboxContext) -> (String, String) {
    crate::system_actor::system_signing_key(&inbox.local_domain, &inbox.ap_private_key_pem)
}

/// DB照合 → 未解決なら1段階だけフェッチを試みて参照を解決する。
/// リプライ/引用/リポストいずれの新規取り込みトップレベル処理からも呼ばれる。
/// フェッチして得たノート自身が持つ参照はさらに辿らず（`resolve_reference_db_only`を使う
/// `save_fetched_remote_note`に委譲するため）、常に1段階で止まる。
pub async fn resolve_reference(
    uri: Option<&str>,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> ReferenceOutcome {
    let Some(uri) = uri else {
        return ReferenceOutcome::None;
    };
    if let Ok(Some(id)) = inbox.post_repo.find_id_by_ap_or_at_uri(uri).await {
        return ReferenceOutcome::Resolved(id);
    }
    let signing_key = system_signing_key(inbox);
    match ap_client
        .fetch_object(uri, (&signing_key.0, &signing_key.1))
        .await
    {
        Ok(note) => match save_fetched_remote_note(note, inbox, ap_client).await {
            Ok(id) => ReferenceOutcome::Resolved(id),
            Err(e) => {
                tracing::warn!("[RefResolve] 参照先の保存に失敗 uri={}: {}", uri, e);
                ReferenceOutcome::Unresolved {
                    ap_uri: uri.to_string(),
                    status: RefStatus::Pending,
                }
            }
        },
        Err(crate::ap::ApError::Gone(detail)) => {
            tracing::info!(
                "[RefResolve] 参照先が消失（404/410） uri={}: {}",
                uri,
                detail
            );
            ReferenceOutcome::Unresolved {
                ap_uri: uri.to_string(),
                status: RefStatus::Gone,
            }
        }
        Err(e) => {
            tracing::warn!("[RefResolve] 参照先フェッチ失敗 uri={}: {}", uri, e);
            ReferenceOutcome::Unresolved {
                ap_uri: uri.to_string(),
                status: RefStatus::Pending,
            }
        }
    }
}

/// 既にフェッチ済みのNote/Questionオブジェクトを`posts`テーブルへ保存する（`fetch_object`の
/// 呼び出し自体は行わない）。`resolve_reference`（リプライ・引用・リポストの参照先を
/// 1段階フェッチする経路）専用の下請け。既存レコードがあれば新規INSERTせずその id を返す。
///
/// 旧`announce.rs`の`fetch_and_save_note`をリプライ/引用からも共用できるよう切り出したもの。
pub(super) async fn save_fetched_remote_note(
    note: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<i64, String> {
    // Note/Question 以外の型（Article 等）は一旦非対応
    if !matches!(note["type"].as_str(), Some("Note") | Some("Question")) {
        return Err(format!(
            "フェッチしたオブジェクトが Note ではありません: type={:?}",
            note["type"]
        ));
    }

    let note_id = note["id"].as_str().unwrap_or_default().to_string();
    if note_id.is_empty() {
        return Err("Note に id がありません".to_string());
    }
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
        let signing_key = system_signing_key(inbox);
        match ap_client
            .fetch_object(&note_id, (&signing_key.0, &signing_key.1))
            .await
        {
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
                    "[RefResolve] 正規Noteからの絵文字tag補完失敗 note_id={}: {}",
                    note_id,
                    error
                );
            }
        }
    }
    record_remote_emojis(inbox, &remote.domain, &tags).await;
    let emoji_map = resolve_emoji_map_with_fallback(inbox, &remote.domain, &tags, &body).await;

    // 引用URI抽出・解決（#116）。ここでは1段階フェッチの対象を広げないため、DB照合のみで解決する。
    let quote_uri = extract_ap_quote_uri(&note, &tags);
    let (quote_of_post_id, quote_of_ap_uri, quote_of_ref_status) =
        resolve_reference_db_only(quote_uri.as_deref(), inbox)
            .await
            .into_parts();
    if let Some(uri) = quote_uri.as_deref() {
        body = strip_quote_fallback_line(&body, uri);
        content_html_sanitized = strip_quote_fallback_line_html(&content_html_sanitized, uri);
    }

    // to/cc から可視性を判定。
    let visibility =
        classify_ap_visibility(&as_string_list(&note["to"]), &as_string_list(&note["cc"]));

    // AP inReplyTo からローカルの reply_to_post_id を解決する（同じくDB照合のみ）。
    let (reply_to_post_id, reply_to_ap_uri, reply_to_ref_status) =
        resolve_reference_db_only(note["inReplyTo"].as_str(), inbox)
            .await
            .into_parts();

    let note_url = note["url"].as_str().unwrap_or("");

    // シナリオ1: ループバックは既存のローカル投稿と同一のため、新規INSERTせずその id を返す。
    if let Some(existing_id) = detect_loopback_post_id(inbox, &note_id, note_url) {
        tracing::warn!(
            "[RefResolve] save_fetched_remote_note: ループバック検知、INSERTをスキップ: note_id={} → 既存post_id={}",
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
            reply_to_ap_uri: reply_to_ap_uri.as_deref(),
            reply_to_ref_status: reply_to_ref_status.map(RefStatus::as_db_str),
            // Announce/リプライ/引用経由でフェッチした投稿がDMであることはAP実装上想定されないため、
            // DMスレッド解決は行わない（direct判定でも thread_root/recipients は空のまま）。
            thread_root_post_id: None,
            recipient_actor_ids: &[],
            quote_of_post_id,
            quote_of_ap_uri: quote_of_ap_uri.as_deref(),
            quote_of_ref_status: quote_of_ref_status.map(RefStatus::as_db_str),
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
            "[RefResolve] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}",
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
                tracing::error!("[RefResolve] 添付 URL 保存失敗（スキップ）: {}", e);
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
        "[RefResolve] 参照先ノートをフェッチして保存: id={}, uri={}",
        saved_id,
        note_id
    );
    Ok(saved_id)
}

/// タイムアウト付きで`pending`な参照を1件解決し、成功または`gone`確定時は`posts`テーブルの
/// 該当行へ結果を書き戻す（#233）。投稿詳細取得時の同期フェッチ・手動「取り込む」APIの
/// 両方から使う。`gone`状態の参照はリトライ対象外のため、呼び出し側で除外してから呼ぶこと。
pub async fn resolve_pending_reference_with_timeout(
    post_id: i64,
    kind: crate::repository::ReferenceKind,
    ap_uri: &str,
    inbox: &InboxContext,
    ap_client: &ApClient,
    timeout: std::time::Duration,
) -> ReferenceOutcome {
    let outcome = match tokio::time::timeout(
        timeout,
        resolve_reference(Some(ap_uri), inbox, ap_client),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            tracing::info!(
                "[RefResolve] pending参照解決がタイムアウト post_id={} uri={}",
                post_id,
                ap_uri
            );
            ReferenceOutcome::Unresolved {
                ap_uri: ap_uri.to_string(),
                status: RefStatus::Pending,
            }
        }
    };

    match &outcome {
        ReferenceOutcome::Resolved(resolved_id) => {
            if let Err(e) = inbox
                .post_repo
                .apply_reference_resolution(post_id, kind, Some(*resolved_id), None)
                .await
            {
                tracing::error!(
                    "[RefResolve] pending参照の解決結果反映に失敗 post_id={} kind={:?}: {}",
                    post_id,
                    kind,
                    e
                );
            }
        }
        // pending→gone（404/410が新たに確認できた）場合のみDBを更新する。まだpendingのまま
        // （一時的失敗・タイムアウト）なら、DB側は既にpendingのため書き戻し不要。
        ReferenceOutcome::Unresolved {
            status: RefStatus::Gone,
            ..
        } => {
            if let Err(e) = inbox
                .post_repo
                .apply_reference_resolution(post_id, kind, None, Some(RefStatus::Gone.as_db_str()))
                .await
            {
                tracing::error!(
                    "[RefResolve] pending→gone反映に失敗 post_id={} kind={:?}: {}",
                    post_id,
                    kind,
                    e
                );
            }
        }
        _ => {}
    }

    outcome
}
