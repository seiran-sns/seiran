//! `seiran_common::repository` の DTO（`TimelinePost`/`Actor`/`NotificationRow`）から
//! Mastodon エンティティへ変換する。付帯情報（添付・リアクション・URL カード・投票状態・
//! リポスト済み判定）は frontend API・Misskey 互換 API と同じ一括取得関数を使う。
//!
//! 「いいね（favourite）」は seiran の `❤️` リアクションに対応させる（Bsky の like・AP の
//! `Like` も受信時に `❤️` リアクションとして保存されるため）。`favourites_count` は絵文字を
//! 問わないリアクション総数（Mastodon クライアントには絵文字リアクションの表示欄が無く、
//! `❤️` だけを数えると絵文字で反応された投稿が無反応に見えるため）。

use std::collections::{HashMap, HashSet};

use seiran_common::repository::{note_extras, Actor, NotificationRow, TimelinePost};

use crate::error::ApiError;
use crate::handlers::notes::delivery::at_uri_to_bsky_app_url;
use crate::handlers::notes::{
    fetch_attachments_map, fetch_link_cards_map, fetch_reactions_map, queries::fetch_reposted_ids,
    resolve_mention_facets_in_place, AttachmentResponse, LinkCardResponse, ReactionSummary,
};
use crate::AppState;

use super::types::{
    MastodonAccount, MastodonAccountSource, MastodonEmoji, MastodonField, MastodonMediaAttachment,
    MastodonMediaMeta, MastodonMediaMetaSize, MastodonNotification, MastodonPoll,
    MastodonPollOption, MastodonQuote, MastodonQuoteApproval, MastodonRelationship, MastodonStatus,
    MastodonTag,
};

/// Mastodon の favourite に対応させるリアクション。
pub const FAVOURITE_REACTION: &str = "❤️";

/// ヘッダー画像未設定時に返す URL（`missing_header` ハンドラ）。Mastodon 本家も未設定時は
/// 実在するプレースホルダー画像の URL を返し、クライアントはそれを前提にデコードする。
pub fn missing_header_url(local_domain: &str) -> String {
    format!("https://{local_domain}/api/headers/missing.png")
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

// ─── 本文・プロフィール文の HTML 化 ─────────────────────────────────────

fn escape_html(s: &str) -> String {
    html_escape::encode_text(s).into_owned()
}

fn escape_attr(s: &str) -> String {
    html_escape::encode_double_quoted_attribute(s).into_owned()
}

/// 内部リンクマーカー（`[表示テキスト](URL)`、`docs/protocols.md` 6節）のうち URL 部分が
/// `/` 始まり（`//` を除く）のものを自インスタンスの絶対 URL にする。Mastodon クライアントは
/// seiran の SPA ルーティングを知らないため、相対パスのままだと開けない。
fn absolutize(url: &str, local_domain: &str) -> String {
    if url.starts_with('/') && !url.starts_with("//") {
        format!("https://{local_domain}{url}")
    } else {
        url.to_owned()
    }
}

/// `text_chars[start] == '['` の位置から `[text](url)` を読む。成立すれば (text, url, 終端)。
fn scan_markdown_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let close = (start + 1..chars.len()).find(|&i| chars[i] == ']' || chars[i] == '\n')?;
    if chars[close] != ']' || chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let url_start = close + 2;
    let url_end =
        (url_start..chars.len()).find(|&i| chars[i] == ')' || chars[i].is_whitespace())?;
    if chars[url_end] != ')' || url_end == url_start {
        return None;
    }
    let text: String = chars[start + 1..close].iter().collect();
    let url: String = chars[url_start..url_end].iter().collect();
    let is_link = url.starts_with("https://")
        || url.starts_with("http://")
        || (url.starts_with('/') && !url.starts_with("//"));
    is_link.then_some((text, url, url_end + 1))
}

/// `text_chars[start] == '@'` の位置からメンション（`@user`・`@user@host`・`@handle.tld`）を
/// 読む。境界規則は `seiran_common::mention` の送信時解決と同じ（直前が半角英数字/`_` なら
/// メールアドレスの一部とみなす）。成立すれば (`@` を除くハンドル, 終端)。
pub(super) fn scan_mention(chars: &[char], start: usize) -> Option<(String, usize)> {
    if start > 0 && (chars[start - 1].is_ascii_alphanumeric() || chars[start - 1] == '_') {
        return None;
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.';
    let mut end = start + 1;
    while end < chars.len() && is_ident(chars[end]) {
        end += 1;
    }
    if end < chars.len() && chars[end] == '@' {
        let mut domain_end = end + 1;
        while domain_end < chars.len()
            && (chars[domain_end].is_alphanumeric()
                || chars[domain_end] == '.'
                || chars[domain_end] == '-')
        {
            domain_end += 1;
        }
        if domain_end > end + 1 {
            end = domain_end;
        }
    }
    // 文末の句読点（`@alice.`）はハンドルに含めない。
    while end > start + 1 && matches!(chars[end - 1], '.' | '-') {
        end -= 1;
    }
    (end > start + 1).then(|| (chars[start + 1..end].iter().collect(), end))
}

fn hashtag_anchor(tag: &str, display: &str, local_domain: &str) -> String {
    format!(
        r#"<a href="https://{}/tags/{}" class="mention hashtag" rel="tag">{}</a>"#,
        local_domain,
        escape_attr(&urlencoding::encode(&tag.to_lowercase())),
        escape_html(display)
    )
}

/// seiran の本文（プレーンテキスト＋内部リンクマーカー）を Mastodon の `content` 形式の
/// HTML にする。生 URL・`[text](url)`・`@メンション`・`#ハッシュタグ` をリンクにし、空行で
/// 段落（`<p>`）、改行で `<br>` に分ける（Mastodon 本家の出力形と同じ構造）。
pub fn text_to_html(text: &str, local_domain: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() * 2);
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut String| {
        out.push_str(&escape_html(plain));
        plain.clear();
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '[' {
            if let Some((label, url, end)) = scan_markdown_link(&chars, i) {
                flush(&mut plain, &mut out);
                match label.strip_prefix('#') {
                    Some(tag) if !tag.is_empty() => {
                        out.push_str(&hashtag_anchor(tag, &label, local_domain));
                    }
                    _ => out.push_str(&format!(
                        r#"<a href="{}" rel="nofollow noopener" target="_blank">{}</a>"#,
                        escape_attr(&absolutize(&url, local_domain)),
                        escape_html(&label)
                    )),
                }
                i = end;
                continue;
            }
        }
        if c == 'h' {
            if let Some(end) = seiran_common::mention::scan_url(&chars, i) {
                flush(&mut plain, &mut out);
                let url: String = chars[i..end].iter().collect();
                out.push_str(&format!(
                    r#"<a href="{0}" rel="nofollow noopener" target="_blank">{1}</a>"#,
                    escape_attr(&url),
                    escape_html(&url)
                ));
                i = end;
                continue;
            }
        }
        if c == '#' {
            if let Some((tag, end)) = seiran_common::mention::scan_hashtag(&chars, i) {
                flush(&mut plain, &mut out);
                out.push_str(&hashtag_anchor(&tag, &format!("#{tag}"), local_domain));
                i = end;
                continue;
            }
        }
        if c == '@' {
            if let Some((handle, end)) = scan_mention(&chars, i) {
                flush(&mut plain, &mut out);
                let username = handle.split('@').next().unwrap_or(&handle);
                out.push_str(&format!(
                    r#"<span class="h-card"><a href="https://{}/@{}" class="u-url mention">@<span>{}</span></a></span>"#,
                    local_domain,
                    escape_attr(&handle),
                    escape_html(username)
                ));
                i = end;
                continue;
            }
        }
        plain.push(c);
        i += 1;
    }
    flush(&mut plain, &mut out);

    out.split("\n\n")
        .filter(|p| !p.is_empty())
        .map(|p| format!("<p>{}</p>", p.replace('\n', "<br>")))
        .collect()
}

/// リモート Fedi 投稿の `content_html`（サニタイズ済み、`docs/protocols.md` 6節）は
/// メンション・ハッシュタグの `href` が seiran 内部パス（`/@user@host`・`/tags/foo`）に
/// 書き換わっているため、絶対 URL に戻す。
fn absolutize_html_links(html: &str, local_domain: &str) -> String {
    html.replace("href=\"/", &format!("href=\"https://{local_domain}/"))
        .replace(&format!("href=\"https://{local_domain}//"), "href=\"//")
}

// ─── 共通の小物 ──────────────────────────────────────────────────────

/// seiran の可視性を Mastodon の語彙にする。
pub fn to_mastodon_visibility(v: &str) -> &'static str {
    match v {
        "unlisted" => "unlisted",
        "followers_only" => "private",
        "direct" => "direct",
        _ => "public",
    }
}

/// Mastodon の可視性を seiran の語彙にする（投稿作成時）。
pub fn from_mastodon_visibility(v: &str) -> Option<&'static str> {
    match v {
        "public" => Some("public"),
        "unlisted" => Some("unlisted"),
        "private" => Some("followers_only"),
        "direct" => Some("direct"),
        _ => None,
    }
}

/// `acct`（Mastodon 本家: ローカルは `username`、リモートは `username@domain`）。
/// Bsky アクターは `domain` が空で `username` がハンドルそのものなので、ハンドルだけにする。
pub fn account_acct(username: &str, domain: &str, actor_type: &str) -> String {
    seiran_common::username::actor_handle(username, domain, actor_type)
        .trim_start_matches('@')
        .to_owned()
}

/// `emoji_map`（`:shortcode:` → URL）群を Mastodon の `emojis` にする（重複は先勝ち）。
fn to_emojis(maps: &[Option<&serde_json::Value>]) -> Vec<MastodonEmoji> {
    let mut seen = HashSet::new();
    maps.iter()
        .flatten()
        .filter_map(|m| m.as_object())
        .flat_map(|m| m.iter())
        .filter_map(|(key, value)| {
            let url = value.as_str()?;
            let shortcode = key
                .strip_prefix(':')
                .and_then(|s| s.strip_suffix(':'))
                .unwrap_or(key)
                .to_owned();
            seen.insert(shortcode.clone()).then(|| MastodonEmoji {
                shortcode,
                url: url.to_owned(),
                static_url: url.to_owned(),
                visible_in_picker: false,
                category: None,
            })
        })
        .collect()
}

// ─── アカウント ──────────────────────────────────────────────────────

/// アカウントの人間向け URL と AP 等の識別子 URI。
fn account_url_uri(actor: &Actor, local_domain: &str) -> (String, String) {
    if actor.actor_type == "local" {
        return (
            format!("https://{local_domain}/@{}", actor.username),
            format!("https://{local_domain}/users/{}", actor.username),
        );
    }
    let fallback = format!(
        "https://{local_domain}/@{}",
        account_acct(&actor.username, &actor.domain, &actor.actor_type)
    );
    let bsky = actor
        .at_did
        .as_deref()
        .map(|did| format!("https://bsky.app/profile/{did}"));
    let url = actor.ap_uri.clone().or(bsky).unwrap_or(fallback);
    let uri = actor
        .ap_uri
        .clone()
        .or_else(|| actor.at_did.as_deref().map(|did| format!("at://{did}")))
        .unwrap_or_else(|| url.clone());
    (url, uri)
}

fn profile_fields(actor: &Actor, local_domain: &str) -> Vec<MastodonField> {
    actor
        .profile_fields
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|fields| {
            fields
                .iter()
                .filter_map(|f| {
                    let name = f["name"].as_str()?;
                    let value = f["value"].as_str().unwrap_or_default();
                    Some(MastodonField {
                        name: name.to_owned(),
                        value: text_to_html(value, local_domain)
                            .trim_start_matches("<p>")
                            .trim_end_matches("</p>")
                            .to_owned(),
                        verified_at: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 複数アクターを Mastodon `Account` にする（アバター・バナー・件数を1クエリで取る）。
/// 戻り値は `actor.id` をキーとするマップ。
pub async fn build_accounts(
    state: &AppState,
    actors: &[Actor],
) -> Result<HashMap<i64, MastodonAccount>, ApiError> {
    if actors.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<i64> = actors.iter().map(|a| a.id).collect();
    let mut media: HashMap<i64, seiran_common::repository::actor::ActorMediaCountsRow> =
        seiran_common::repository::actor::media_and_counts_for_actors(&state.db, &ids)
            .await
            .map_err(internal)?
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
    let local_domain = state.local_domain.as_str();
    Ok(actors
        .iter()
        .map(|actor| {
            let m = media.remove(&actor.id);
            let avatar = m
                .as_ref()
                .and_then(|m| m.avatar_url.clone())
                .unwrap_or_else(|| {
                    seiran_common::avatar::fallback_avatar_url(local_domain, actor.id)
                });
            let header = m
                .as_ref()
                .and_then(|m| m.banner_url.clone())
                .unwrap_or_else(|| missing_header_url(local_domain));
            let (url, uri) = account_url_uri(actor, local_domain);
            let account = MastodonAccount {
                id: actor.id.to_string(),
                username: actor.username.clone(),
                acct: account_acct(&actor.username, &actor.domain, &actor.actor_type),
                url,
                uri,
                display_name: actor.display_name.clone().unwrap_or_default(),
                note: text_to_html(actor.bio.as_deref().unwrap_or_default(), local_domain),
                avatar_static: avatar.clone(),
                avatar,
                header_static: header.clone(),
                header,
                locked: actor.is_locked,
                bot: false,
                group: false,
                discoverable: true,
                indexable: true,
                created_at: m
                    .as_ref()
                    .map(|m| m.created_at)
                    .unwrap_or_else(chrono::Utc::now)
                    .to_rfc3339(),
                last_status_at: None,
                statuses_count: m.as_ref().map_or(0, |m| m.notes_count),
                followers_count: m.as_ref().map_or(0, |m| m.followers_count),
                following_count: m.as_ref().map_or(0, |m| m.following_count),
                emojis: to_emojis(&[actor.emoji_map.as_ref()]),
                fields: profile_fields(actor, local_domain),
                source: None,
            };
            (actor.id, account)
        })
        .collect())
}

pub async fn build_account(state: &AppState, actor: &Actor) -> Result<MastodonAccount, ApiError> {
    build_accounts(state, std::slice::from_ref(actor))
        .await?
        .remove(&actor.id)
        .ok_or(ApiError::NotFound("ACCOUNT_NOT_FOUND"))
}

/// アクターID群から `Account` を組み立てる（見つからないIDは結果に含まれない）。
pub async fn build_accounts_by_ids(
    state: &AppState,
    ids: &[i64],
) -> Result<HashMap<i64, MastodonAccount>, ApiError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let actors = state.actors.find_by_ids(ids).await.map_err(internal)?;
    build_accounts(state, &actors).await
}

/// `ids` の順序を保って `Account` の配列にする（見つからないIDは除く）。
pub async fn build_accounts_ordered(
    state: &AppState,
    ids: &[i64],
) -> Result<Vec<MastodonAccount>, ApiError> {
    let mut by_id = build_accounts_by_ids(state, ids).await?;
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// `verify_credentials`（自分自身）用。`source` を足す。
pub async fn build_credential_account(
    state: &AppState,
    actor: &Actor,
) -> Result<MastodonAccount, ApiError> {
    let mut account = build_account(state, actor).await?;
    let follow_requests_count = state
        .follows
        .count_pending(actor.id)
        .await
        .map_err(internal)?;
    account.source = Some(MastodonAccountSource {
        privacy: "public",
        sensitive: false,
        language: None,
        note: actor.bio.clone().unwrap_or_default(),
        fields: account.fields.clone(),
        follow_requests_count,
    });
    Ok(account)
}

/// 閲覧者 `viewer` から見た各アクターとの関係（`GET /api/v1/accounts/relationships`）。
/// 入力の順序を保つ。
pub async fn build_relationships(
    state: &AppState,
    viewer: i64,
    ids: &[i64],
) -> Result<Vec<MastodonRelationship>, ApiError> {
    let (fwd, rev, blocks, muted, renote_muted) = tokio::join!(
        state.follows.find_statuses_among(viewer, ids),
        state.follows.find_statuses_by_followers_among(viewer, ids),
        state.blocks.find_relationships_among(viewer, ids),
        state.mutes.list_muted_among(viewer, ids),
        state.repost_mutes.list_muted_among(viewer, ids),
    );
    let (fwd, rev, blocks, muted, renote_muted) = (
        fwd.map_err(internal)?,
        rev.map_err(internal)?,
        blocks.map_err(internal)?,
        muted.map_err(internal)?,
        renote_muted.map_err(internal)?,
    );
    Ok(ids
        .iter()
        .map(|&id| {
            let fwd_status = fwd.get(&id).map(String::as_str);
            let rev_status = rev.get(&id).map(String::as_str);
            let (blocking, blocked_by) = blocks.get(&id).copied().unwrap_or((false, false));
            let is_muted = muted.contains(&id);
            MastodonRelationship {
                id: id.to_string(),
                following: fwd_status == Some("accepted"),
                showing_reblogs: !renote_muted.contains(&id),
                notifying: false,
                languages: None,
                followed_by: rev_status == Some("accepted"),
                blocking,
                blocked_by,
                muting: is_muted,
                muting_notifications: is_muted,
                requested: fwd_status == Some("pending"),
                requested_by: rev_status == Some("pending"),
                domain_blocking: false,
                endorsed: false,
                note: String::new(),
            }
        })
        .collect())
}

// ─── ステータス ──────────────────────────────────────────────────────

/// 複数の投稿を変換するのに必要な付帯情報（投稿ID・アクターID単位の一括取得結果）。
#[derive(Default)]
struct StatusMaterials {
    accounts: HashMap<i64, MastodonAccount>,
    attachments: HashMap<i64, Vec<AttachmentResponse>>,
    reactions: HashMap<i64, Vec<ReactionSummary>>,
    link_cards: HashMap<i64, Vec<LinkCardResponse>>,
    poll_votes: HashMap<i64, Vec<i32>>,
    /// 閲覧者がリポスト済みの投稿ID（未ログインなら `None`）。
    reposted: Option<HashSet<i64>>,
    /// 返信先投稿の投稿者ID（`in_reply_to_account_id`）。
    reply_authors: HashMap<i64, i64>,
    /// 閲覧者がブックマーク済み・ピン留め済みの投稿ID。
    bookmarked: HashSet<i64>,
    pinned: HashSet<i64>,
}

impl StatusMaterials {
    async fn fetch(
        state: &AppState,
        rows: &[TimelinePost],
        viewer: Option<i64>,
    ) -> Result<Self, ApiError> {
        let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
        let mut actor_ids: Vec<i64> = rows.iter().map(|p| p.actor_id).collect();
        actor_ids.sort_unstable();
        actor_ids.dedup();
        let mut reply_ids: Vec<i64> = rows.iter().filter_map(|p| p.reply_to_post_id).collect();
        reply_ids.sort_unstable();
        reply_ids.dedup();

        let (bookmarked, pinned) = match viewer {
            Some(v) => {
                let (b, p) = tokio::join!(
                    seiran_common::repository::bookmark::bookmarked_among(&state.db, v, &ids),
                    state.pinned_posts.list_by_actor(v),
                );
                (
                    b.map_err(internal)?.into_iter().collect(),
                    p.map_err(internal)?.into_iter().collect(),
                )
            }
            None => (HashSet::new(), HashSet::new()),
        };
        let (accounts, attachments, reactions, link_cards, poll_votes, reposted, reply_rows) = tokio::join!(
            build_accounts_by_ids(state, &actor_ids),
            fetch_attachments_map(&state.db, &ids),
            fetch_reactions_map(&state.db, &ids, viewer),
            fetch_link_cards_map(&state.db, &ids),
            async {
                match viewer {
                    Some(v) if !ids.is_empty() => {
                        note_extras::poll_votes_by_actor(&state.db, v, &ids).await
                    }
                    _ => Ok(Vec::new()),
                }
            },
            async {
                match viewer {
                    Some(v) => Some(fetch_reposted_ids(&state.db, v, &ids).await),
                    None => None,
                }
            },
            seiran_common::repository::find_visible_posts_by_ids(&state.db, &reply_ids, viewer),
        );
        let mut votes: HashMap<i64, Vec<i32>> = HashMap::new();
        for (post_id, index) in poll_votes.map_err(internal)? {
            votes.entry(post_id).or_default().push(index);
        }
        Ok(Self {
            accounts: accounts?,
            attachments,
            reactions,
            link_cards,
            poll_votes: votes,
            reposted,
            reply_authors: reply_rows
                .map_err(internal)?
                .into_iter()
                .map(|p| (p.id, p.actor_id))
                .collect(),
            bookmarked,
            pinned,
        })
    }
}

fn to_media_attachment(
    post_id: i64,
    index: usize,
    a: &AttachmentResponse,
) -> MastodonMediaAttachment {
    let kind = if a.is_gif {
        "gifv"
    } else if a.mime_type.starts_with("image/") {
        "image"
    } else if a.mime_type.starts_with("video/") {
        "video"
    } else if a.mime_type.starts_with("audio/") {
        "audio"
    } else {
        "unknown"
    };
    let original = (a.width > 0 && a.height > 0).then(|| MastodonMediaMetaSize {
        width: a.width,
        height: a.height,
        size: format!("{}x{}", a.width, a.height),
        aspect: f64::from(a.width) / f64::from(a.height),
    });
    MastodonMediaAttachment {
        id: format!("{post_id}{index:02}"),
        kind,
        url: a.url.clone(),
        preview_url: a.thumbnail_url.clone().unwrap_or_else(|| a.url.clone()),
        remote_url: None,
        meta: MastodonMediaMeta {
            small: original.clone(),
            original,
        },
        description: None,
        blurhash: None,
    }
}

fn to_card(card: &LinkCardResponse) -> serde_json::Value {
    serde_json::json!({
        "url": card.url,
        "title": card.title,
        "description": card.description,
        "type": if card.embed_src.is_some() { "video" } else { "link" },
        "author_name": "",
        "author_url": "",
        "provider_name": "",
        "provider_url": "",
        "html": "",
        "width": 0,
        "height": 0,
        "image": card.thumbnail_url,
        "embed_url": card.embed_src.clone().unwrap_or_default(),
        "blurhash": null,
    })
}

/// `posts.poll`（`{"multiple", "options": [{"name", "votes"}], "endTime"}`）を `Poll` にする。
fn to_poll(
    post_id: i64,
    poll: &serde_json::Value,
    own_votes: &[i32],
    viewer: bool,
) -> Option<MastodonPoll> {
    let options: Vec<MastodonPollOption> = poll["options"]
        .as_array()?
        .iter()
        .map(|o| MastodonPollOption {
            title: o["name"].as_str().unwrap_or_default().to_owned(),
            votes_count: o["votes"].as_i64().unwrap_or(0),
        })
        .collect();
    let expires_at = poll["endTime"].as_str().map(str::to_owned);
    let expired = expires_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|t| t < chrono::Utc::now());
    Some(MastodonPoll {
        id: post_id.to_string(),
        expires_at,
        expired,
        multiple: poll["multiple"].as_bool().unwrap_or(false),
        votes_count: options.iter().map(|o| o.votes_count).sum(),
        voters_count: None,
        options,
        emojis: Vec::new(),
        voted: viewer && !own_votes.is_empty(),
        own_votes: own_votes.to_vec(),
    })
}

/// 変換済みステータスと、埋め込み対象（リポスト元・引用元）の投稿ID。
struct Converted {
    status: MastodonStatus,
    reblog_of: Option<i64>,
    quote_of: Option<i64>,
    /// リポストだが元投稿が未取り込み（`repost_of_ap_uri` のみ）等で `reblog_of` が無いもの。
    is_repost: bool,
}

fn to_status(
    p: &TimelinePost,
    local_domain: &str,
    viewer: Option<i64>,
    m: &StatusMaterials,
) -> Option<Converted> {
    let account = m.accounts.get(&p.actor_id)?.clone();
    let is_local = p.actor_type == "local";
    let local_url = format!("https://{local_domain}/notes/{}", p.id);
    let uri = if is_local {
        local_url.clone()
    } else {
        p.post_ap_object_id
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| p.post_at_uri.clone())
            .unwrap_or_else(|| local_url.clone())
    };
    let url = if is_local {
        Some(local_url)
    } else {
        p.post_ap_object_id
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| p.post_at_uri.as_deref().map(at_uri_to_bsky_app_url))
    };

    let is_repost = p.repost_of_post_id.is_some() || p.repost_of_ap_uri.is_some();
    let attachments = m.attachments.get(&p.id).map(Vec::as_slice).unwrap_or(&[]);
    let reactions = m.reactions.get(&p.id).map(Vec::as_slice).unwrap_or(&[]);
    let own_votes = m.poll_votes.get(&p.id).map(Vec::as_slice).unwrap_or(&[]);

    let content = if is_repost {
        String::new()
    } else if let Some(html) = p.content_html.as_deref().filter(|h| !h.is_empty()) {
        absolutize_html_links(html, local_domain)
    } else {
        text_to_html(&p.body, local_domain)
    };
    let tags = if is_repost {
        Vec::new()
    } else {
        seiran_common::hashtag::extract_hashtags(&p.body)
            .into_iter()
            .map(|name| MastodonTag {
                url: format!("https://{local_domain}/tags/{}", urlencoding::encode(&name)),
                name,
                history: Vec::new(),
                following: false,
            })
            .collect()
    };
    let visibility = to_mastodon_visibility(&p.visibility);
    let quotable = matches!(visibility, "public" | "unlisted");

    let status = MastodonStatus {
        id: p.id.to_string(),
        uri,
        url,
        created_at: p.created_at.to_rfc3339(),
        edited_at: None,
        account,
        content,
        visibility,
        sensitive: p.content_warning.is_some() || attachments.iter().any(|a| a.is_sensitive),
        spoiler_text: p.content_warning.clone().unwrap_or_default(),
        media_attachments: attachments
            .iter()
            .enumerate()
            .map(|(i, a)| to_media_attachment(p.id, i, a))
            .collect(),
        application: None,
        mentions: Vec::new(),
        tags,
        emojis: to_emojis(&[p.post_emoji_map.as_ref(), p.actor_emoji_map.as_ref()]),
        reblogs_count: p.repost_count,
        favourites_count: reactions.iter().map(|r| r.count).sum(),
        replies_count: p.reply_count,
        quotes_count: p.quote_count,
        in_reply_to_id: p.reply_to_post_id.map(|id| id.to_string()),
        in_reply_to_account_id: p
            .reply_to_post_id
            .and_then(|id| m.reply_authors.get(&id))
            .map(|id| id.to_string()),
        reblog: None,
        quote: None,
        quote_approval: MastodonQuoteApproval {
            automatic: if quotable { vec!["public"] } else { Vec::new() },
            manual: Vec::new(),
            current_user: match (viewer, quotable) {
                (None, _) => "unknown",
                (Some(_), true) => "automatic",
                (Some(_), false) => "denied",
            },
        },
        poll: p
            .poll
            .as_ref()
            .and_then(|poll| to_poll(p.id, poll, own_votes, viewer.is_some())),
        card: m
            .link_cards
            .get(&p.id)
            .and_then(|cards| cards.first())
            .map(to_card),
        language: None,
        text: None,
        filtered: Vec::new(),
        favourited: viewer.map(|_| {
            reactions
                .iter()
                .any(|r| r.reacted_by_me && r.emoji == FAVOURITE_REACTION)
        }),
        reblogged: m.reposted.as_ref().map(|set| set.contains(&p.id)),
        muted: viewer.map(|_| false),
        bookmarked: viewer.map(|_| m.bookmarked.contains(&p.id)),
        pinned: viewer.map(|_| m.pinned.contains(&p.id)),
    };
    Some(Converted {
        status,
        reblog_of: p.repost_of_post_id,
        quote_of: p.quote_of_post_id,
        is_repost,
    })
}

/// 参照先の埋め込みをせずに変換する（入力の順序を保つ。投稿者が取れない行は除く）。
async fn convert_rows(
    state: &AppState,
    mut rows: Vec<TimelinePost>,
    viewer: Option<i64>,
) -> Result<Vec<Converted>, ApiError> {
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let materials = StatusMaterials::fetch(state, &rows, viewer).await?;
    Ok(rows
        .iter()
        .filter_map(|p| to_status(p, &state.local_domain, viewer, &materials))
        .collect())
}

/// 投稿ID群を可視性判定つきで取り出して変換し、id → `Converted` にする。
async fn fetch_converted(
    state: &AppState,
    ids: &[i64],
    viewer: Option<i64>,
) -> Result<HashMap<i64, Converted>, ApiError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = seiran_common::repository::find_visible_posts_by_ids(&state.db, ids, viewer)
        .await
        .map_err(internal)?;
    Ok(convert_rows(state, rows, viewer)
        .await?
        .into_iter()
        .filter_map(|c| c.status.id.parse::<i64>().ok().map(|id| (id, c)))
        .collect())
}

fn referenced_ids<'a>(items: impl Iterator<Item = &'a Converted>) -> Vec<i64> {
    let mut ids: Vec<i64> = items
        .flat_map(|c| [c.reblog_of, c.quote_of])
        .flatten()
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn quote_of(target: Option<i64>, found: Option<MastodonStatus>) -> Option<MastodonQuote> {
    target.map(|_| MastodonQuote {
        state: if found.is_some() {
            "accepted"
        } else {
            "deleted"
        },
        quoted_status: found.map(Box::new),
    })
}

/// リポスト元（`reblog`）と引用元（`quote`）を埋め込む。引用ポストのリポストでは
/// リポスト元がさらに引用を持つので、2階層目の引用まで埋める（それより深くは埋めない）。
async fn embed_references(
    state: &AppState,
    items: Vec<Converted>,
    viewer: Option<i64>,
) -> Result<Vec<Converted>, ApiError> {
    let mut level1 = fetch_converted(state, &referenced_ids(items.iter()), viewer).await?;
    let level2 = fetch_converted(state, &referenced_ids(level1.values()), viewer).await?;
    for c in level1.values_mut() {
        c.status.quote = quote_of(
            c.quote_of,
            c.quote_of
                .and_then(|id| level2.get(&id))
                .map(|q| q.status.clone()),
        );
        c.status.reblog = c
            .reblog_of
            .and_then(|id| level2.get(&id))
            .map(|r| Box::new(r.status.clone()));
    }
    Ok(items
        .into_iter()
        .map(|mut c| {
            c.status.quote = quote_of(
                c.quote_of,
                c.quote_of
                    .and_then(|id| level1.get(&id))
                    .map(|q| q.status.clone()),
            );
            if let Some(reblog) = c.reblog_of.and_then(|id| level1.get(&id)) {
                c.status.reblogged = reblog.status.reblogged;
                c.status.favourited = reblog.status.favourited;
                c.status.reblog = Some(Box::new(reblog.status.clone()));
            }
            c
        })
        .collect())
}

/// タイムライン等、複数の投稿を Mastodon `Status` にする（入力の順序を保つ）。
/// リポスト元が見えない（削除済み・非公開・未取り込み）リポストは空の投稿として表示されて
/// しまうため除く。
pub async fn build_statuses(
    state: &AppState,
    rows: Vec<TimelinePost>,
    viewer: Option<i64>,
) -> Result<Vec<MastodonStatus>, ApiError> {
    let items = convert_rows(state, rows, viewer).await?;
    Ok(embed_references(state, items, viewer)
        .await?
        .into_iter()
        .filter(|c| !c.is_repost || c.status.reblog.is_some())
        .map(|c| c.status)
        .collect())
}

pub async fn build_status(
    state: &AppState,
    post: TimelinePost,
    viewer: Option<i64>,
) -> Result<MastodonStatus, ApiError> {
    build_statuses(state, vec![post], viewer)
        .await?
        .pop()
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))
}

/// 投稿IDから、閲覧者に見える投稿を `Status` にする。
pub async fn find_status(
    state: &AppState,
    post_id: i64,
    viewer: Option<i64>,
) -> Result<MastodonStatus, ApiError> {
    let post = state
        .posts
        .find_by_id_for_viewer(post_id, viewer)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))?;
    build_status(state, post, viewer).await
}

// ─── 通知 ────────────────────────────────────────────────────────────

/// seiran の通知種別を Mastodon の `type` にする。Mastodon に相当物の無い種別（アカウント
/// 引っ越しの再フォロー通知等）は `None`（一覧から除く）。
fn to_mastodon_notification_type(kind: &str, reaction: Option<&str>) -> Option<&'static str> {
    Some(match kind {
        "follow" => "follow",
        "followRequest" => "follow_request",
        "mention" | "reply" => "mention",
        "repost" => "reblog",
        "quote" => "quote",
        "reaction" if reaction == Some(FAVOURITE_REACTION) => "favourite",
        "reaction" => "pleroma:emoji_reaction",
        _ => return None,
    })
}

/// 通知一覧を Mastodon 形式にする。`recipient` は通知の宛先本人（ステータスの閲覧者視点）。
pub async fn build_notifications(
    state: &AppState,
    rows: Vec<NotificationRow>,
    recipient: i64,
) -> Result<Vec<MastodonNotification>, ApiError> {
    let rows: Vec<(NotificationRow, &'static str)> = rows
        .into_iter()
        .filter_map(|r| {
            let kind = to_mastodon_notification_type(&r.kind, r.reaction.as_deref())?;
            r.notifier_actor_id.is_some().then_some((r, kind))
        })
        .collect();

    let mut actor_ids: Vec<i64> = rows
        .iter()
        .filter_map(|(r, _)| r.notifier_actor_id)
        .collect();
    actor_ids.sort_unstable();
    actor_ids.dedup();
    let mut post_ids: Vec<i64> = rows.iter().filter_map(|(r, _)| r.note_id).collect();
    post_ids.sort_unstable();
    post_ids.dedup();

    let (accounts, posts) = tokio::join!(
        build_accounts_by_ids(state, &actor_ids),
        seiran_common::repository::find_visible_posts_by_ids(&state.db, &post_ids, Some(recipient)),
    );
    let accounts = accounts?;
    let statuses: HashMap<i64, MastodonStatus> =
        build_statuses(state, posts.map_err(internal)?, Some(recipient))
            .await?
            .into_iter()
            .filter_map(|s| s.id.parse::<i64>().ok().map(|id| (id, s)))
            .collect();

    Ok(rows
        .into_iter()
        .filter_map(|(r, kind)| {
            let account = accounts.get(&r.notifier_actor_id?)?.clone();
            let status = r.note_id.and_then(|id| statuses.get(&id)).cloned();
            // 「リポストされた」通知の `status` は Mastodon ではリポストされた自分の投稿。
            // seiran の通知はリポストのラッパー投稿を指すので、その中身を取り出す。
            let status = if kind == "reblog" {
                status.and_then(|s| s.reblog.map(|b| *b))
            } else {
                status
            };
            if kind != "follow" && kind != "follow_request" && status.is_none() {
                return None;
            }
            let is_emoji_reaction = kind == "pleroma:emoji_reaction";
            Some(MastodonNotification {
                id: r.id.to_string(),
                kind,
                created_at: r.created_at.to_rfc3339(),
                account,
                status,
                emoji: r.reaction.clone().filter(|_| is_emoji_reaction),
                emoji_url: r.reaction_emoji_url.clone().filter(|_| is_emoji_reaction),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "seiran.example";

    #[test]
    fn plain_text_becomes_paragraphs_with_line_breaks() {
        assert_eq!(text_to_html("a\nb\n\nc", D), "<p>a<br>b</p><p>c</p>");
    }

    #[test]
    fn html_special_characters_are_escaped() {
        assert_eq!(
            text_to_html("<b>&</b>", D),
            "<p>&lt;b&gt;&amp;&lt;/b&gt;</p>"
        );
    }

    #[test]
    fn raw_url_is_linked() {
        assert_eq!(
            text_to_html("see https://example.com/a?b=1 now", D),
            r#"<p>see <a href="https://example.com/a?b=1" rel="nofollow noopener" target="_blank">https://example.com/a?b=1</a> now</p>"#
        );
    }

    #[test]
    fn internal_link_marker_with_relative_path_becomes_absolute() {
        assert_eq!(
            text_to_html("[見て](/notes/1)", D),
            r#"<p><a href="https://seiran.example/notes/1" rel="nofollow noopener" target="_blank">見て</a></p>"#
        );
    }

    #[test]
    fn hashtag_links_to_local_tag_page() {
        assert_eq!(
            text_to_html("今日も#猫", D),
            format!(
                r#"<p>今日も<a href="https://seiran.example/tags/{}" class="mention hashtag" rel="tag">#猫</a></p>"#,
                urlencoding::encode("猫")
            )
        );
    }

    #[test]
    fn markdown_hashtag_link_points_to_local_tag_page() {
        let html = text_to_html("[#Foo](https://remote.example/tags/foo)", D);
        assert!(
            html.contains(r#"href="https://seiran.example/tags/foo""#),
            "{html}"
        );
    }

    #[test]
    fn mentions_link_to_local_profile_pages() {
        let html = text_to_html("@alice と @bob@remote.example と @carol.bsky.social.", D);
        assert!(
            html.contains(r#"href="https://seiran.example/@alice""#),
            "{html}"
        );
        assert!(
            html.contains(r#"href="https://seiran.example/@bob@remote.example""#),
            "{html}"
        );
        assert!(
            html.contains(r#"href="https://seiran.example/@carol.bsky.social""#),
            "{html}"
        );
        assert!(html.ends_with(".</p>"), "{html}");
    }

    #[test]
    fn email_address_is_not_a_mention() {
        assert_eq!(text_to_html("a@b.example", D), "<p>a@b.example</p>");
    }

    #[test]
    fn content_html_relative_links_become_absolute() {
        assert_eq!(
            absolutize_html_links(
                r#"<a href="/@bob@x.example">@bob</a><a href="https://y/">y</a>"#,
                D
            ),
            r#"<a href="https://seiran.example/@bob@x.example">@bob</a><a href="https://y/">y</a>"#
        );
    }

    #[test]
    fn visibility_round_trips() {
        for v in ["public", "unlisted", "followers_only", "direct"] {
            assert_eq!(from_mastodon_visibility(to_mastodon_visibility(v)), Some(v));
        }
    }

    #[test]
    fn acct_omits_domain_for_local_and_bsky() {
        assert_eq!(account_acct("alice", D, "local"), "alice");
        assert_eq!(
            account_acct("carol.bsky.social", "", "bsky"),
            "carol.bsky.social"
        );
        assert_eq!(
            account_acct("bob", "remote.example", "fedi"),
            "bob@remote.example"
        );
    }

    #[test]
    fn heart_reaction_notification_is_favourite() {
        assert_eq!(
            to_mastodon_notification_type("reaction", Some("❤️")),
            Some("favourite")
        );
        assert_eq!(
            to_mastodon_notification_type("reaction", Some(":blob:")),
            Some("pleroma:emoji_reaction")
        );
        assert_eq!(
            to_mastodon_notification_type("repost", None),
            Some("reblog")
        );
        assert_eq!(to_mastodon_notification_type("moveRefollowed", None), None);
    }
}
