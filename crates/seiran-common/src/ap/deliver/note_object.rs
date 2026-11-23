//! ローカル投稿の AP Note（アンケート付きなら Question）オブジェクトを組み立てる唯一の場所。
//!
//! Create・Update・DM の配送と `GET /notes/:id`（受信側が URL から直接取り込む経路）は、
//! すべてここで DB から組み立てた同じオブジェクトを使う。経路ごとに組み立てると
//! 片方にだけフィールドが入らないずれが起きる（`GET /notes/:id` に `inReplyTo` が無く、
//! URL から取り込んだ返信が通常ノートになった件など）ため、ジョブにも呼び出し元にも
//! 本文や返信先を持たせず、投稿 ID だけを受け取る。

use super::activity::{
    append_emoji_tags, build_note_object, build_seiran_post_for_basis, fetch_post_activity_basis,
    html_and_tags_for_body, NoteActivityParams,
};
use super::infra::{fetch_post_reference_ids, local_actor_address};
use super::text::{at_uri_to_bsky_app_url, plain_to_html};
use super::*;
use crate::repository::post::PostDeliveryMeta;
use crate::repository::{PgPostRepository, PostRepository};

/// 投稿の出自（どちらのプロトコルにネイティブ実体を持つか）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostOrigin {
    /// ローカルまたは他 seiran（AP/ATP 両方の実体を持つ）
    LocalOrSeiran,
    /// Fedi リモート（AP 実体のみ）
    FediRemote,
    /// Bsky リモート（ATP 実体のみ）
    BskyRemote,
}

pub fn classify_post(
    ap_object_id: Option<&str>,
    at_uri: Option<&str>,
    is_local: bool,
) -> PostOrigin {
    if is_local {
        return PostOrigin::LocalOrSeiran;
    }
    match (ap_object_id.is_some(), at_uri.is_some()) {
        // seiran リモート: ap_object_id あり AND at_uri あり（かつ domain != local）
        (true, true) => PostOrigin::LocalOrSeiran,
        // Fedi リモート: ap_object_id あり AND at_uri なし
        (true, false) => PostOrigin::FediRemote,
        // Bsky リモート: ap_object_id なし AND at_uri あり
        (false, true) => PostOrigin::BskyRemote,
        // 判定不能 → ローカル相当として扱う
        (false, false) => PostOrigin::LocalOrSeiran,
    }
}

/// brid.gyブリッジポスト対応（`crate::bridge_post`・`docs/protocols.md`参照）:
/// `id`が指すポストがブリッジポスト（`meta.bridge_of_post_id`が`Some`）の場合、元ポストの
/// メタ情報を再取得して返す（内部的には常に元ポストへのリポスト/引用として扱うため、
/// 呼び出し元は返り値の`i64`を`repost_of_post_id`/`quote_of_post_id`にそのまま使うこと）。
/// 非ブリッジ、または元ポストの取得に失敗した場合は引数をそのまま返す。
pub async fn redirect_bridge_post_meta(
    posts: &dyn PostRepository,
    id: i64,
    meta: PostDeliveryMeta,
) -> (i64, PostDeliveryMeta) {
    match meta.bridge_of_post_id {
        Some(original_id) => match posts.find_delivery_meta(original_id).await {
            Ok(Some(original_meta)) => (original_id, original_meta),
            _ => (id, meta),
        },
        None => (id, meta),
    }
}

/// brid.gyブリッジポスト対応: `meta`（元ポスト、`redirect_bridge_post_meta`適用済みである
/// こと）が対向プロトコル側に独立したブリッジポストを持つ場合、配送先識別子
/// （ap_object_id/at_uri/at_cid）をそちらへ差し替える。`origin == LocalOrSeiran`
/// （元ポストが両プロトコルにネイティブ実体を持つ、seiran自身の投稿）の場合は、わざわざ
/// ブリッジポストへ送る必要が無いため差し替えない（ユーザー合意済み仕様）。
pub async fn with_bridge_delivery_targets(
    posts: &dyn PostRepository,
    meta: PostDeliveryMeta,
    origin: PostOrigin,
) -> PostDeliveryMeta {
    if origin == PostOrigin::LocalOrSeiran {
        return meta;
    }
    let mut meta = meta;
    if let Some(bridge_id) = meta.ap_bridge_post_id {
        if let Ok(Some(bridge_meta)) = posts.find_delivery_meta(bridge_id).await {
            meta.ap_object_id = bridge_meta.ap_object_id;
        }
    }
    if let Some(bridge_id) = meta.atp_bridge_post_id {
        if let Ok(Some(bridge_meta)) = posts.find_delivery_meta(bridge_id).await {
            meta.at_uri = bridge_meta.at_uri;
            meta.at_cid = bridge_meta.at_cid;
        }
    }
    meta
}

/// 引用・リポスト先のメタ情報を、ブリッジポストの付け替えまで済ませて取得する。
/// 取得できなければ`None`。
pub async fn find_delivery_meta_with_bridge(
    posts: &dyn PostRepository,
    id: i64,
) -> Option<(PostDeliveryMeta, PostOrigin)> {
    let meta = posts.find_delivery_meta(id).await.ok().flatten()?;
    let (_, meta) = redirect_bridge_post_meta(posts, id, meta).await;
    let origin = classify_post(
        meta.ap_object_id.as_deref(),
        meta.at_uri.as_deref(),
        meta.actor_type == "local",
    );
    let meta = with_bridge_delivery_targets(posts, meta, origin).await;
    Some((meta, origin))
}

/// Fedi 配送で使う引用表現。
#[derive(Debug, PartialEq, Eq)]
pub enum ApQuote {
    /// AP 実体を持つ投稿は Misskey 互換の quoteUrl として配送する。
    Misskey(String),
    /// Bsky にしか実体がない投稿は、受信サーバーが AP オブジェクトとして解決できないため、
    /// bsky.app URL を本文末尾へ追記する。
    AppendUrl(String),
}

pub fn ap_quote_from_meta(meta: &PostDeliveryMeta) -> Option<ApQuote> {
    if meta.at_uri.is_some() && meta.ap_object_id.is_none() {
        meta.at_uri
            .as_deref()
            .map(at_uri_to_bsky_app_url)
            .map(ApQuote::AppendUrl)
    } else {
        meta.ap_object_id.clone().map(ApQuote::Misskey)
    }
}

/// Fedi 配送する本文（平文）を決める。Bsky にしか実体が無い引用先は bsky.app URL を追記し、
/// リンクカード（`post_link_cards`）の URL のうち本文に無いものを末尾へ追記する。
/// 返り値は（本文, quoteUrl）。
pub fn fedi_body_and_quote_url(
    body: &str,
    quote: Option<ApQuote>,
    link_card_urls: &[String],
) -> (String, Option<String>) {
    let (mut text, quote_url) = match quote {
        Some(ApQuote::Misskey(url)) => (body.to_string(), Some(url)),
        Some(ApQuote::AppendUrl(url)) => (format!("{}\n\n{}", body, url), None),
        None => (body.to_string(), None),
    };
    let mut append_lines: Vec<&str> = Vec::new();
    for url in link_card_urls {
        if !text.contains(url.as_str()) && !append_lines.contains(&url.as_str()) {
            append_lines.push(url);
        }
    }
    if !append_lines.is_empty() {
        text = format!("{}\n\n{}", text, append_lines.join("\n"));
    }
    (text, quote_url)
}

/// Bsky にしか実体が無い投稿のリポストを Fedi へ届けるときの代替本文。
pub fn repost_fallback_text(original: &PostDeliveryMeta) -> Option<String> {
    let at_uri = original.at_uri.as_deref()?;
    let author_name = original
        .display_name
        .as_deref()
        .unwrap_or(&original.username);
    Some(format!(
        "🔁 {}: {}",
        author_name,
        at_uri_to_bsky_app_url(at_uri)
    ))
}

/// `build_local_note`の結果。`object`は`@context`を含まない Note/Question オブジェクト。
pub struct LocalNote {
    pub object: serde_json::Value,
    pub username: String,
    pub visibility: String,
    /// 本文中でメンションしたアクターURI一覧（配送先の追加に使う）。
    pub mention_uris: Vec<String>,
    /// `seiranPost.counterpartPostId`（この投稿の ATP 側実体）が入っているか。
    pub has_counterpart: bool,
}

/// ローカル投稿`post_id`（投稿者`actor_id`）の Note を DB から組み立てる。
///
/// リポストラッパーのうち、元ポストが AP 実体を持つもの（Announce で配送するもの）は
/// Note ではないため`None`を返す。Bsky にしか実体が無い元ポストのリポストは、
/// 代替本文（`repost_fallback_text`）の Note を返す。
pub async fn build_local_note(
    ap_client: &ApClient,
    db: &PgPool,
    local_domain: &str,
    post_id: i64,
    actor_id: i64,
) -> Result<Option<LocalNote>, ApError> {
    let basis = fetch_post_activity_basis(db, post_id, actor_id).await?;
    let refs = fetch_post_reference_ids(db, post_id).await?;
    let posts = PgPostRepository::new(db.clone());
    let is_direct = basis.visibility == "direct";

    let mut repost_fallback: Option<String> = None;
    if let Some(repost_of) = refs.repost_of_post_id {
        let Some((meta, _)) = find_delivery_meta_with_bridge(&posts, repost_of).await else {
            return Ok(None);
        };
        if meta.ap_object_id.is_some() {
            return Ok(None);
        }
        let Some(text) = repost_fallback_text(&meta) else {
            return Ok(None);
        };
        repost_fallback = Some(text);
    }

    let in_reply_to = match refs.reply_to_post_id {
        Some(id) => posts
            .find_delivery_meta(id)
            .await
            .map_err(|e| ApError::Other(format!("返信先取得エラー: {}", e)))?
            .and_then(|m| m.ap_object_id),
        None => None,
    };

    let (content_html, tag, mention_uris, seiran_post, quote_url) = match repost_fallback {
        // 投稿者本人が書いた本文ではない合成テキストのため、メンション変換も
        // 「投稿の完全再現」（seiranPost）もしない。
        Some(text) => (plain_to_html(&text), Vec::new(), Vec::new(), None, None),
        None => {
            let quote = match refs.quote_of_post_id {
                Some(id) => find_delivery_meta_with_bridge(&posts, id)
                    .await
                    .and_then(|(meta, _)| ap_quote_from_meta(&meta)),
                None => None,
            };
            let link_card_urls: Vec<String> =
                crate::seiran_post::fetch_attachments_and_link_cards(db, post_id)
                    .await
                    .map_err(|e| ApError::Other(format!("リンクカード取得エラー: {}", e)))?
                    .1
                    .into_iter()
                    .map(|c| c.url)
                    .collect();
            let (body, quote_url) = fedi_body_and_quote_url(&basis.body, quote, &link_card_urls);
            let (content_html, mut tag, mention_uris, converted_body) =
                html_and_tags_for_body(&body, local_domain, db, ap_client).await;
            append_emoji_tags(&body, &basis.emoji_map, &mut tag, local_domain);
            // DM（`chat.bsky.convo`経由、`commit_post`を通らない）は`posts.at_uri`が
            // 設定されないためcounterpartPostIdを埋める見込みが無く、対象外とする。
            let seiran_post = if is_direct {
                None
            } else {
                build_seiran_post_for_basis(db, post_id, &basis, &converted_body).await?
            };
            (content_html, tag, mention_uris, seiran_post, quote_url)
        }
    };

    let direct_recipients = if is_direct {
        fetch_direct_recipient_uris(db, post_id).await?
    } else {
        Vec::new()
    };
    let has_counterpart = seiran_post
        .as_ref()
        .is_some_and(|sp| sp.counterpart_post_id.is_some());

    let addr = local_actor_address(local_domain, &basis.username);
    let object = build_note_object(
        &addr,
        &NoteActivityParams {
            local_domain,
            post_id,
            content_html: &content_html,
            published: &basis.created_at.to_rfc3339(),
            attachments: basis.attachments,
            quote_url: quote_url.as_deref(),
            in_reply_to: in_reply_to.as_deref(),
            seiran_uuid: if is_direct {
                None
            } else {
                basis.seiran_uuid.as_deref()
            },
            seiran_post,
            visibility: &basis.visibility,
            tag,
            direct_recipients: &direct_recipients,
            // directは`direct_recipients`が既に実際の宛先そのものなので無視される（visibility_to_to_cc参照）。
            mention_recipients: &mention_uris,
            poll: basis.poll.as_ref(),
            content_warning: basis.content_warning.as_deref(),
        },
    );

    Ok(Some(LocalNote {
        object,
        username: basis.username,
        visibility: basis.visibility,
        mention_uris,
        has_counterpart,
    }))
}

/// `build_local_note`の Note を包む Create アクティビティ（配送と outbox で同じ形）。
pub fn local_note_create_activity(
    local_domain: &str,
    post_id: i64,
    note: &LocalNote,
) -> serde_json::Value {
    let addr = local_actor_address(local_domain, &note.username);
    super::activity::build_create_note_activity(&addr, local_domain, post_id, &note.object)
}

/// DM の宛先のうち Fedi アクターの AP URI 一覧（`to`に入れる）。
async fn fetch_direct_recipient_uris(db: &PgPool, post_id: i64) -> Result<Vec<String>, ApError> {
    let rows = sqlx::query(
        "SELECT a.ap_uri
         FROM post_recipients pr JOIN actors a ON a.id = pr.actor_id
         WHERE pr.post_id = $1 AND a.actor_type IN ('fedi', 'remote_seiran') AND a.ap_uri IS NOT NULL AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("DM宛先取得エラー: {}", e)))?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_uri").ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fedi_body_plain() {
        assert_eq!(
            fedi_body_and_quote_url("本文", None, &[]),
            ("本文".to_string(), None)
        );
    }

    #[test]
    fn fedi_body_misskey_quote_keeps_body() {
        assert_eq!(
            fedi_body_and_quote_url(
                "本文",
                Some(ApQuote::Misskey("https://a.example/notes/1".into())),
                &[]
            ),
            (
                "本文".to_string(),
                Some("https://a.example/notes/1".to_string())
            )
        );
    }

    #[test]
    fn fedi_body_bsky_only_quote_appends_url() {
        assert_eq!(
            fedi_body_and_quote_url(
                "本文",
                Some(ApQuote::AppendUrl(
                    "https://bsky.app/profile/x/post/y".into()
                )),
                &[]
            ),
            (
                "本文\n\nhttps://bsky.app/profile/x/post/y".to_string(),
                None
            )
        );
    }

    #[test]
    fn fedi_body_appends_only_link_cards_missing_from_text() {
        let urls = vec![
            "https://example.com/in-text".to_string(),
            "https://example.com/a".to_string(),
            "https://example.com/b".to_string(),
        ];
        assert_eq!(
            fedi_body_and_quote_url("見て https://example.com/in-text", None, &urls).0,
            "見て https://example.com/in-text\n\nhttps://example.com/a\nhttps://example.com/b"
        );
    }

    #[test]
    fn fedi_body_link_card_check_includes_appended_quote_url() {
        let quote_url = "https://bsky.app/profile/x/post/y".to_string();
        assert_eq!(
            fedi_body_and_quote_url(
                "本文",
                Some(ApQuote::AppendUrl(quote_url.clone())),
                std::slice::from_ref(&quote_url)
            )
            .0,
            "本文\n\nhttps://bsky.app/profile/x/post/y"
        );
    }

    #[test]
    fn classify_post_local_domain_match() {
        // is_local=true の場合は ap_object_id / at_uri の値によらずローカル扱い
        assert_eq!(classify_post(None, None, true), PostOrigin::LocalOrSeiran);
    }

    #[test]
    fn classify_post_seiran_remote_has_both_ids() {
        assert_eq!(
            classify_post(Some("https://a/notes/1"), Some("at://did/x/y"), false),
            PostOrigin::LocalOrSeiran
        );
    }

    #[test]
    fn classify_post_fedi_remote_ap_only() {
        assert_eq!(
            classify_post(Some("https://mastodon.example/notes/1"), None, false),
            PostOrigin::FediRemote
        );
    }

    #[test]
    fn classify_post_bsky_remote_at_uri_only() {
        assert_eq!(
            classify_post(None, Some("at://did/x/y"), false),
            PostOrigin::BskyRemote
        );
    }

    #[test]
    fn classify_post_unknown_defaults_to_local() {
        assert_eq!(classify_post(None, None, false), PostOrigin::LocalOrSeiran);
    }
}
