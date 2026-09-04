use super::content::{
    strip_quote_fallback_line_html, strip_quote_fallback_line_html_leading,
    strip_quote_inline_paragraph_html,
};
use super::emoji::{
    has_same_origin, has_unresolved_emoji_shortcodes, record_remote_emojis,
    resolve_emoji_map_with_fallback,
};
use super::note_input::{
    detect_loopback_post_id, extract_ap_quote_uri, guess_attachment_mime_type, normalize_ap_poll,
    resolve_bridge_duplicate_post_id, strip_quote_fallback_line, strip_quote_fallback_line_leading,
};
use super::reference::{resolve_ref, system_signing_key, ReferenceResolutionMode};
use super::*;

/// `save_ap_note_core`が新規INSERTした場合の結果。呼び出し側固有の後処理
/// （通知生成はCreate直接受信のみ）に必要な値だけを束ねる。
pub(super) struct SavedApNote {
    pub post_id: i64,
    pub note_id: String,
    pub actor_id: i64,
    pub remote: RemoteActorInfo,
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub emoji_map: serde_json::Value,
    pub visibility: &'static str,
    pub reply_to_post_id: Option<i64>,
    pub quote_of_post_id: Option<i64>,
    pub recipient_actor_ids: Vec<i64>,
    /// 絵文字tag補完後の最終tag配列。メンション通知先解決（Create直接受信のみ）に使う。
    pub tags: Vec<serde_json::Value>,
    pub parent_original_post_id: Option<i64>,
}

/// `save_ap_note_core`の結果。
pub(super) enum SaveApNoteOutcome {
    /// 新規にINSERTした。
    Inserted(Box<SavedApNote>),
    /// 既にDBに存在した（ap_object_id重複／seiran_uuidマージ／ループバック検知のいずれか）ため
    /// 新規INSERTを行わなかった。呼び出し側は通知・配信を行わず、この post_id をそのまま使う。
    AlreadyExists { post_id: i64 },
}

/// AP Note/Questionをpostsテーブルへ保存する唯一の共通処理。Create直接受信
/// （`handle_create_note`）・参照解決経由（`save_fetched_remote_note`、リプライ先/引用元/
/// リポスト対象の1段階フェッチ）のどちらから呼ばれても、アクター解決／絵文字解決／
/// 引用・リプライ参照解決／可視性判定／（`OneHopFetch`時のみ）DM宛先・スレッド起点解決／
/// seiran_uuidマージ／ループバック・ブリッジ重複検知／DB挿入／メタデータ更新／
/// ハッシュタグリンク／OGPリンクカードのenqueue／添付メディア保存を必ず同じ手順で実行する。
/// 通知生成・WebSocket配信は呼び出し側の責務（Create直接受信でのみ行う）。
pub(super) async fn save_ap_note_core(
    note: &serde_json::Value,
    actor_uri: &str,
    inbox: &InboxContext,
    ap_client: &ApClient,
    ref_mode: ReferenceResolutionMode,
) -> Result<SaveApNoteOutcome, String> {
    if !matches!(note["type"].as_str(), Some("Note") | Some("Question")) {
        return Err(format!(
            "フェッチしたオブジェクトが Note ではありません: type={:?}",
            note["type"]
        ));
    }
    let note_id = note["id"]
        .as_str()
        .ok_or("Note に id がありません")?
        .to_string();

    // 同一Noteの再処理では投稿本体だけでなく引用・返信・メンション通知も二重生成しない。
    // insert_remote_with_dedupのON CONFLICTより前に終了する。
    if let Some(existing) = inbox
        .post_repo
        .find_id_by_ap_or_at_uri(&note_id)
        .await
        .map_err(|e| format!("重複チェック失敗: {}", e))?
    {
        return Ok(SaveApNoteOutcome::AlreadyExists { post_id: existing });
    }

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

    // `seiranPost`拡張オブジェクト（#237）。検出できた場合、標準フィールドのベストエフォート
    // 変換（このすぐ下から続く既存パイプライン）は変わらず計算されるが、下記の各所で
    // その結果を上書きする（無ければ標準変換のフォールバックのまま使う設計、
    // `docs/protocols.md` 5節）。
    let seiran_post_ext = crate::seiran_post::SeiranPost::extract(note);

    let mut tags = note["tag"].as_array().cloned().unwrap_or_default();

    // kmyblue系は`<p class="quote-inline">RE: <a>URL</a></p>`を本文の**先頭**（Fedibird/
    // Misskeyが本文末尾に付ける`RE:`/`QT:`行とは逆の位置）に自動挿入する（実例:
    // kblue.10rino.net等）。`class`属性は`sanitize_ap_content_html`が全タグから剥がすため、
    // Markdown化・サニタイズより前の生HTMLの段階で検出・除去する。tag補完前の`tags`で
    // 十分（`quote`/`_misskey_quote`/`quoteUri`はNote直下フィールドでtag補完の対象外）。
    let early_quote_uri = extract_ap_quote_uri(note, &tags);
    let content_html_pre = early_quote_uri
        .as_deref()
        .and_then(|uri| {
            // 第一候補: `class="quote-inline"`（Mastodon/kmyblue系の標準的なマーカー）。
            // 第二候補: classが無い場合でも、先頭ブロックがテキストベースで
            // `RE:`/`QT:`フォールバックと判定できれば除去する。
            strip_quote_inline_paragraph_html(&content_html, uri).or_else(|| {
                let leading = strip_quote_fallback_line_html_leading(&content_html, uri);
                (leading != content_html).then_some(leading)
            })
        })
        .unwrap_or_else(|| content_html.clone());

    // HTML タグを除去して本文を得る（<a href> はリンクとして保持し、Markdownリンク記法
    // `[text](url)` に変換する。メンションは `@user@host` のプレーンテキストに正規化）。
    let mut body = ap_content_to_markdown_body(&content_html_pre, &tags, &remote.domain);
    // seiran Web UI でのリッチ表示用（`<blockquote>`/`<ruby>`等の構造保持、#233）。
    // `body`とは別に、意味的構造をクレンジングして保持したHTMLを`content_html`列に持つ。
    let mut content_html_sanitized =
        sanitize_ap_content_html(&content_html_pre, &tags, &remote.domain);
    // リレー実装によっては、配送する Create の埋め込み Note から Emoji tag を
    // 省略する一方、object.id の正規 Note には完全な tag を載せる。本文に未解決の
    // shortcode がある場合だけ正規 Note を取得し、欠落した tag を補完する。
    // object.id は外部入力なので、解決済み投稿者actorと同一originの場合だけ取得する。
    if has_unresolved_emoji_shortcodes(&tags, &body) && has_same_origin(&note_id, actor_uri) {
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
                    body = ap_content_to_markdown_body(&content_html_pre, &tags, &remote.domain);
                    content_html_sanitized =
                        sanitize_ap_content_html(&content_html_pre, &tags, &remote.domain);
                }
            }
            Err(error) => {
                tracing::warn!(
                    "[NoteSave] 正規Noteからの絵文字tag補完失敗 note_id={}: {}",
                    note_id,
                    error
                );
            }
        }
    }
    // 本文中のカスタム絵文字（`:shortcode:`）→画像URLマップ（AP Note の tag 配列由来）。
    record_remote_emojis(inbox, &remote.domain, &tags).await;
    let emoji_map = resolve_emoji_map_with_fallback(inbox, &remote.domain, &tags, &body).await;

    // 引用URI抽出・解決（#116）。`OneHopFetch`ならDBに無ければ1段階だけフェッチを試みる
    // （#231）。取得できた場合、Misskey/Fedibirdが本文末尾に自動付加する`RE:`/`QT:`
    // フォールバック行（引用URIと同じURLを指す）を本文から取り除く。kmyblueの先頭
    // `quote-inline`段落は上の`content_html_pre`計算時に既に除去済みのため、ここでは
    // 除去し損ねた場合（`class`が付かない・末尾に来るkmyblueの別表記等）のフォールバック
    // として働く。tag補完後の最終`tags`で`quote_uri`を再計算する（`early_quote_uri`は
    // 補完前の値のため、tag[].relフォールバックの結果がtag補完で変わる場合がある）。
    let quote_uri = extract_ap_quote_uri(note, &tags);
    let (quote_of_post_id, quote_of_ap_uri, quote_of_ref_status) =
        resolve_ref(ref_mode, quote_uri.as_deref(), inbox, ap_client)
            .await
            .into_parts();
    if let Some(uri) = quote_uri.as_deref() {
        // 末尾（Fedibird/Misskey）と先頭（kmyblue）の両方を確認する。上の`content_html_pre`
        // 計算で先頭段落を除去できていれば、ここでの先頭側呼び出しは通常no-opになる保険。
        body = strip_quote_fallback_line(&body, uri);
        body = strip_quote_fallback_line_leading(&body, uri);
        content_html_sanitized = strip_quote_fallback_line_html(&content_html_sanitized, uri);
        content_html_sanitized =
            strip_quote_fallback_line_html_leading(&content_html_sanitized, uri);
    }
    // to/cc から可視性を判定（#配送先・可視性アイコン追加）。
    let to_list = as_string_list(&note["to"]);
    let visibility = classify_ap_visibility(&to_list, &as_string_list(&note["cc"]));
    // seiranPostがあれば申告された可視性で上書きする（不明な値ならAP標準の判定を使う）。
    let visibility = seiran_post_ext
        .as_ref()
        .and_then(|sp| match sp.visibility.as_str() {
            "public" => Some("public"),
            "unlisted" => Some("unlisted"),
            "followers_only" => Some("followers_only"),
            "direct" => Some("direct"),
            _ => None,
        })
        .unwrap_or(visibility);

    // AP inReplyTo からローカルの reply_to_post_id を解決する（DM機能実装以前はこの解決自体が
    // 存在しなかった。通常投稿にも有用だが、direct（DM）のスレッド起点伝播に必須のため追加）。
    let (reply_to_post_id, reply_to_ap_uri, reply_to_ref_status) =
        resolve_ref(ref_mode, note["inReplyTo"].as_str(), inbox, ap_client)
            .await
            .into_parts();

    // DM（visibility="direct"）の宛先・スレッド起点解決。`OneHopFetch`（＝トップレベル
    // 受信）の時だけ行う。参照解決経由（`DbOnly`）でフェッチしたNoteは実際にはinboxへ
    // 配送されていないため、DM宛先情報を信頼してはならない（意図的に常にスキップ）。
    let (thread_root_post_id, recipient_actor_ids): (Option<i64>, Vec<i64>) =
        if ref_mode == ReferenceResolutionMode::OneHopFetch && visibility == "direct" {
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
                let Some(local_username) =
                    crate::ap::extract_local_username(uri, &inbox.local_domain)
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

    // シナリオ2: 他seiranサーバー間マージ（#237、相互一致方式）。
    // `seiranPost.counterpartPostId`（ATP側の真正なat_uri申告）がある場合のみ、
    // `insert_remote_with_dedup`へ`claimed_at_uri`として渡す。実際の相互一致判定
    // （advisory lock・既存行の逆申告確認・投稿者一貫性チェック）はそちら側
    // （`repository::post::insert_remote_with_dedup`）が担う。旧`seiran_post_uuid`方式
    // （内部限定UUIDの単純一致のみで投稿者確認が無く、他人の投稿を乗っ取れる欠陥があった）
    // はこの方式に置き換えられ廃止した。
    let claimed_at_uri = seiran_post_ext
        .as_ref()
        .and_then(|sp| sp.counterpart_post_id.as_deref());
    let seiran_uuid: Option<&str> = None;

    let note_url = note["url"].as_str().unwrap_or("");

    // シナリオ1: ループバックは既存のローカル投稿の重複でしかないため、新規INSERTせず無視する。
    if let Some(existing_id) = detect_loopback_post_id(inbox, &note_id, note_url) {
        tracing::warn!(
            "[NoteSave] ループバック検知、INSERTをスキップ: note_id={} → 既存post_id={}",
            note_id,
            existing_id
        );
        return Ok(SaveApNoteOutcome::AlreadyExists {
            post_id: existing_id,
        });
    }

    let parent_original_post_id = resolve_bridge_duplicate_post_id(inbox, note_url).await;

    // seiranPostがあれば本文・絵文字マップを標準変換の代わりに使う（Single Source of
    // Truth、`docs/protocols.md` 5節）。添付・URLカードの完全再現（isSensitive/isGif等）は
    // 本パスでは未対応で、標準AP添付フィールド（`note["attachment"]`）のベストエフォート
    // フォールバックのまま（追って対応予定）。
    let body = seiran_post_ext
        .as_ref()
        .map(|sp| sp.body.clone())
        .unwrap_or(body);
    let emoji_map = seiran_post_ext
        .as_ref()
        .map(|sp| sp.emoji_map.clone())
        .unwrap_or(emoji_map);
    let content_html_for_insert: Option<&str> = if seiran_post_ext.is_some() {
        None
    } else {
        Some(&content_html_sanitized)
    };

    // posts テーブルに挿入（ap_object_id 重複はスキップ、claimed_at_uriがあれば
    // #237相互一致マージを試みる。旧seiran_uuidは常にNone、詳細は上記コメント参照）。
    inbox
        .post_repo
        .insert_remote_with_dedup(InsertRemoteWithDedupParams {
            id: post_id,
            actor_id,
            body: &body,
            content_html: content_html_for_insert,
            ap_object_id: &note_id,
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
            claimed_at_uri,
        })
        .await
        .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    // ON CONFLICTで既存行だった場合を含め、DB上の真のidを取得する（自前生成idが
    // 実在しない行を指してしまう事故を防ぐ）。
    let post_id = inbox
        .post_repo
        .find_id_by_ap_or_at_uri(&note_id)
        .await
        .map_err(|e| format!("posts id 取得エラー: {}", e))?
        .ok_or_else(|| format!("posts id 取得エラー: {} が見つかりません", note_id))?;

    let (content_warning, poll) = match &seiran_post_ext {
        Some(sp) => (sp.content_warning.as_deref(), sp.poll.clone()),
        None => (
            note["summary"].as_str().filter(|s| !s.is_empty()),
            normalize_ap_poll(note),
        ),
    };
    inbox
        .post_repo
        .set_fedi_content_metadata(post_id, content_warning, poll.as_ref())
        .await
        .map_err(|e| format!("投稿メタデータ更新エラー: {}", e))?;

    if let Err(e) = inbox.hashtag_repo.link_post(post_id, &body).await {
        tracing::error!(
            "[NoteSave] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}",
            e
        );
    }

    // URLカード（OGP取得ジョブがoEmbed discoveryによる埋め込みプレーヤー解決も行う）。
    queue_link_cards_for_post(&inbox.queue, post_id, &body).await;

    // 添付画像・動画・音声の URL を保存（S3 には保存せず URL のみ記録）
    save_remote_attachments(inbox, post_id, note).await;

    Ok(SaveApNoteOutcome::Inserted(Box::new(SavedApNote {
        post_id,
        note_id,
        actor_id,
        remote,
        body,
        created_at,
        emoji_map,
        visibility,
        reply_to_post_id,
        quote_of_post_id,
        recipient_actor_ids,
        tags,
        parent_original_post_id,
    })))
}

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
/// Create直接受信・参照解決経由（リプライ先/引用元/リポスト対象の1段階フェッチ）の
/// どちらから保存された投稿でも`save_ap_note_core`から必ず呼ばれる。
pub(super) async fn queue_link_cards_for_post(queue: &Arc<dyn JobQueue>, post_id: i64, body: &str) {
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
