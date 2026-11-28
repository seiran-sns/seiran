//! Fedibird 互換の絵文字リアクション（`fedibird_capabilities` の `emoji_reaction`）。
//!
//! Fedibird・kmyblue に対応したクライアント（Scia 等）は、`/api/v1/instance` の
//! `fedibird_capabilities` に `emoji_reaction` があるサーバーで、投稿の `emoji_reactions` を
//! 表示し、`PUT`/`DELETE /api/v1/statuses/:id/emoji_reactions/:name` で付け外しする。
//!
//! - seiran のリアクションは 1投稿1人1つ（Misskey と同じ）なので、別の絵文字を付けると前の
//!   リアクションと置き換わる。`❤️` は Mastodon のお気に入りにあたるので、`emoji_reactions`・
//!   `emoji_reactioned_by`・ストリーミングの `emoji_reaction` には含めない。
//! - Fedibird は絵文字を `name` でまとめ、ほかのサーバーのカスタム絵文字は `name` を
//!   ショートコードだけにして `domain` を別に持つ。同じショートコードならサーバーが違っても
//!   1つのリアクションとして扱う（クライアントも `name` だけで見分ける）ので、ここでも
//!   `name` でまとめる。

use std::collections::HashSet;

use axum::{
    extract::{OriginalUri, Path, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use seiran_common::repository::{
    is_valid_emoji_shortcode, note_extras, parse_reaction_shortcode_and_host,
};
use seiran_common::streaming::NoteUpdatedBody;

use crate::error::ApiError;
use crate::handlers::notes::{ReactRequest, ReactionSummary};
use crate::middleware::{AuthedUser, MaybeAuthedUser};
use crate::AppState;

use super::convert::{build_accounts_by_ids, find_status, FAVOURITE_REACTION};
use super::extract::{id_cursors, paginated, MastodonQuery, PageParams};
use super::types::MastodonAccount;

/// 投稿の `emoji_reactions` の1件（Fedibird の `REST::GroupedEmojiReactionSerializer`）。
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct FedibirdReaction {
    /// Unicode の絵文字か、カスタム絵文字のショートコード（サーバーを含まない）。
    pub name: String,
    pub count: i64,
    /// 閲覧者がこの絵文字を付けているか。未ログインでは省く。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub me: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_url: Option<String>,
    /// ほかのサーバーのカスタム絵文字なら、そのサーバー。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// 通知の `emoji_reaction` と `emoji_reactioned_by` の絵文字（Fedibird の
/// `REST::EmojiReactionSerializer` の絵文字の部分）。
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct FedibirdReactionEmoji {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// 通知の `emoji_reaction`（Fedibird の `REST::NotificationSerializer::EmojiReactionSerializer`）。
/// 投稿の `emoji_reactions` と同じ形を `count`・`me` まで含めて返す。Feather 等は両者を同じ型で
/// 読むので、`count` が無いと通知一覧全体のデコードに失敗する。通知1件はリアクション1件なので
/// `count` は 1、`me` は Fedibird と同じく常に `false`（付けたのは通知の相手）。
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct FedibirdNotificationReaction {
    #[serde(flatten)]
    pub emoji: FedibirdReactionEmoji,
    pub count: i64,
    pub me: bool,
}

impl From<FedibirdReactionEmoji> for FedibirdNotificationReaction {
    fn from(emoji: FedibirdReactionEmoji) -> Self {
        Self {
            emoji,
            count: 1,
            me: false,
        }
    }
}

/// ストリーミングの `emoji_reaction` イベントの本文（Fedibird の
/// `PublishEmojiReactionWorker`）。その絵文字の件数を、届いた値に置き換えさせる。
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct FedibirdReactionUpdate {
    pub status_id: String,
    pub name: String,
    pub count: i64,
    /// 付けた人の ID。seiran は投稿ごとに付けた人を集計しないので、受け手本人が付けて
    /// いればその ID だけを入れる（クライアントは自分が付けたかをこれで決める）。
    pub account_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// `emoji_reactioned_by` の1件。
#[derive(Serialize, Clone, Debug)]
pub struct FedibirdReactedBy {
    #[serde(flatten)]
    pub emoji: FedibirdReactionEmoji,
    pub account: MastodonAccount,
}

/// `reactions.content` を Fedibird の `name` と `domain` に分ける。自サーバーのカスタム絵文字
/// （`:shortcode@.:`・旧形式の `:shortcode:`・自ドメイン付き）は `domain` なし。
pub fn split_content(content: &str, local_domain: &str) -> (String, Option<String>) {
    match parse_reaction_shortcode_and_host(content) {
        Some((shortcode, None | Some("."))) => (shortcode.to_owned(), None),
        Some((shortcode, Some(host))) if host.eq_ignore_ascii_case(local_domain) => {
            (shortcode.to_owned(), None)
        }
        Some((shortcode, Some(host))) => (shortcode.to_owned(), Some(host.to_owned())),
        None => (content.to_owned(), None),
    }
}

/// リアクション1件の絵文字を Fedibird の形にする。
pub fn reaction_emoji(
    content: &str,
    emoji_url: Option<&str>,
    local_domain: &str,
) -> FedibirdReactionEmoji {
    let (name, domain) = split_content(content, local_domain);
    FedibirdReactionEmoji {
        name,
        url: emoji_url.map(str::to_owned),
        static_url: emoji_url.map(str::to_owned),
        domain,
    }
}

/// 投稿のリアクション集計を、お気に入り（`❤️`）を除いて `name` ごとにまとめる。並びは
/// 入力（件数の多い順）のまま、同じ `name` は最初に出たものの画像とサーバーを使う。
pub fn to_fedibird_reactions(
    reactions: &[ReactionSummary],
    local_domain: &str,
    with_me: bool,
) -> Vec<FedibirdReaction> {
    let mut out: Vec<FedibirdReaction> = Vec::new();
    for r in reactions.iter().filter(|r| r.emoji != FAVOURITE_REACTION) {
        let emoji = reaction_emoji(&r.emoji, r.emoji_url.as_deref(), local_domain);
        match out.iter_mut().find(|g| g.name == emoji.name) {
            Some(group) => {
                group.count += r.count;
                group.me = group.me.map(|me| me || r.reacted_by_me);
                if group.url.is_none() {
                    group.url = emoji.url;
                    group.static_url = emoji.static_url;
                    group.domain = emoji.domain;
                }
            }
            None => out.push(FedibirdReaction {
                name: emoji.name,
                count: r.count,
                me: with_me.then_some(r.reacted_by_me),
                url: emoji.url,
                static_url: emoji.static_url,
                domain: emoji.domain,
            }),
        }
    }
    out
}

/// パスの `:name` を、付けるリアクションの内容にする。Fedibird はほかのサーバーのカスタム
/// 絵文字を `shortcode@domain` で受け取る。seiran では同名の自サーバーの絵文字として付く
/// （自サーバーに無ければ `UNKNOWN_EMOJI`。`notes::validation::validate_reaction_content`）。
fn content_from_name(name: &str) -> String {
    let name = name.trim();
    if name.len() > 2 && name.starts_with(':') && name.ends_with(':') {
        return name.to_owned();
    }
    match name.split_once('@') {
        Some((shortcode, host)) if is_valid_emoji_shortcode(shortcode) && !host.is_empty() => {
            format!(":{shortcode}@{host}:")
        }
        _ if is_valid_emoji_shortcode(name) => format!(":{name}:"),
        _ => name.to_owned(),
    }
}

/// パスの `:name` を、比べるための `name`（サーバーとコロンを除いたもの）にする。
fn bare_name(name: &str) -> &str {
    let name = name.trim();
    let inner = name
        .strip_prefix(':')
        .and_then(|s| s.strip_suffix(':'))
        .unwrap_or(name);
    match inner.split_once('@') {
        Some((shortcode, _)) if is_valid_emoji_shortcode(shortcode) => shortcode,
        _ => inner,
    }
}

fn parse_id(id: &str) -> Result<i64, ApiError> {
    id.parse()
        .map_err(|_| ApiError::NotFound("RECORD_NOT_FOUND"))
}

async fn status_response(state: &AppState, post_id: i64, viewer: i64) -> Response {
    match find_status(state, post_id, Some(viewer)).await {
        Ok(status) => Json(status).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PUT /api/v1/statuses/:id/emoji_reactions/:name` — 絵文字でリアクションする（既に別の絵文字を
/// 付けていれば置き換わる）。リアクション後の `Status` を返す。
pub async fn react(
    user: AuthedUser,
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> Response {
    let viewer = user.actor_id;
    let post_id = match parse_id(&id) {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let resp = crate::handlers::notes::create_reaction(
        Path(id),
        user,
        State(state.clone()),
        Json(ReactRequest {
            content: content_from_name(&name),
        }),
    )
    .await;
    if !resp.status().is_success() {
        return resp;
    }
    status_response(&state, post_id, viewer).await
}

/// `DELETE /api/v1/statuses/:id/emoji_reactions/:name` — 自分のその絵文字のリアクションを外す。
/// Fedibird と同じく `name` で照合し、付けていなくても現在の `Status` を返す。
pub async fn unreact(
    user: AuthedUser,
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> Response {
    let viewer = user.actor_id;
    let post_id = match parse_id(&id) {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let own = match note_extras::own_reactions_for_posts(&state.db, viewer, &[post_id]).await {
        Ok(rows) => rows,
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };
    let wanted = bare_name(&name);
    let target = own
        .into_iter()
        .map(|(_, content)| content)
        .find(|content| split_content(content, &state.local_domain).0 == wanted);
    if let Some(content) = target {
        let resp =
            crate::handlers::notes::reactions::remove_reaction(&state, &user, &id, Some(&content))
                .await;
        if !resp.status().is_success() && resp.status() != axum::http::StatusCode::NOT_FOUND {
            return resp;
        }
    }
    status_response(&state, post_id, viewer).await
}

/// `GET /api/v1/statuses/:id/emoji_reactioned_by` — 投稿に絵文字でリアクションした人を、
/// 絵文字と組にして新しい順に返す（絵文字では絞らない。Fedibird と同じ）。
pub async fn reactioned_by(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    Path(id): Path<String>,
    MastodonQuery(page): MastodonQuery<PageParams>,
) -> Result<Response, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    let post_id = parse_id(&id)?;
    state
        .posts
        .find_by_id_for_viewer(post_id, viewer)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))?;
    let rows = state
        .reactions
        .list_for_post(post_id, FAVOURITE_REACTION, viewer, page.page(40, 80))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let ids: Vec<String> = rows.iter().map(|r| r.id.to_string()).collect();
    let cursors = id_cursors(ids.iter().map(String::as_str));
    let mut actor_ids: Vec<i64> = rows.iter().map(|r| r.actor_id).collect();
    actor_ids.sort_unstable();
    actor_ids.dedup();
    let accounts = build_accounts_by_ids(&state, &actor_ids).await?;
    let items: Vec<FedibirdReactedBy> = rows
        .into_iter()
        .filter_map(|r| {
            Some(FedibirdReactedBy {
                emoji: reaction_emoji(&r.content, r.emoji_url.as_deref(), &state.local_domain),
                account: accounts.get(&r.actor_id)?.clone(),
            })
        })
        .collect();
    Ok(paginated(&uri, &state.local_domain, items, cursors))
}

/// ストリーミングの `noteUpdated` を、受け手 `viewer` 向けの `emoji_reaction` イベントの本文に
/// する。いま付いている絵文字すべてと、切り替え・取り消しで消えた絵文字（0件）を送る。
/// `viewer_reaction` は受け手が付けているリアクション（呼び出し側が引く）。
pub fn reaction_updates(
    body: &NoteUpdatedBody,
    viewer: i64,
    viewer_reaction: Option<&str>,
    local_domain: &str,
) -> Vec<FedibirdReactionUpdate> {
    let viewer_name = viewer_reaction
        .filter(|c| *c != FAVOURITE_REACTION)
        .map(|c| split_content(c, local_domain).0);
    let summaries: Vec<ReactionSummary> = body
        .reactions
        .iter()
        .map(|r| ReactionSummary {
            emoji: r.emoji.clone(),
            count: r.count,
            reacted_by_me: false,
            emoji_url: r.emoji_url.clone(),
        })
        .collect();
    let mut updates: Vec<FedibirdReactionUpdate> =
        to_fedibird_reactions(&summaries, local_domain, false)
            .into_iter()
            .map(|g| FedibirdReactionUpdate {
                status_id: body.post_id.clone(),
                account_ids: account_ids_for(&g.name, viewer, viewer_name.as_deref()),
                name: g.name,
                count: g.count,
                url: g.url,
                static_url: g.static_url,
                domain: g.domain,
            })
            .collect();
    let present: HashSet<String> = updates.iter().map(|u| u.name.clone()).collect();
    if let Some(previous) = body
        .reactor_previous_emoji
        .as_deref()
        .filter(|c| *c != FAVOURITE_REACTION)
    {
        let (name, domain) = split_content(previous, local_domain);
        if !present.contains(&name) {
            updates.push(FedibirdReactionUpdate {
                status_id: body.post_id.clone(),
                account_ids: Vec::new(),
                name,
                count: 0,
                url: None,
                static_url: None,
                domain,
            });
        }
    }
    updates
}

fn account_ids_for(name: &str, viewer: i64, viewer_name: Option<&str>) -> Vec<String> {
    if viewer_name == Some(name) {
        vec![viewer.to_string()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiran_common::streaming::ReactionCount;

    const D: &str = "seiran.example";

    fn summary(emoji: &str, count: i64, me: bool, url: Option<&str>) -> ReactionSummary {
        ReactionSummary {
            emoji: emoji.to_owned(),
            count,
            reacted_by_me: me,
            emoji_url: url.map(str::to_owned),
        }
    }

    #[test]
    fn content_splits_into_name_and_domain() {
        assert_eq!(split_content("👍", D), ("👍".to_owned(), None));
        assert_eq!(split_content(":blob@.:", D), ("blob".to_owned(), None));
        assert_eq!(split_content(":blob:", D), ("blob".to_owned(), None));
        assert_eq!(
            split_content(":blob@seiran.example:", D),
            ("blob".to_owned(), None)
        );
        assert_eq!(
            split_content(":blob@misskey.example:", D),
            ("blob".to_owned(), Some("misskey.example".to_owned()))
        );
    }

    #[test]
    fn favourite_is_excluded_and_same_name_is_merged() {
        let reactions = [
            summary("❤️", 5, true, None),
            summary(
                ":blob@misskey.example:",
                2,
                false,
                Some("https://m/blob.png"),
            ),
            summary(":blob@.:", 1, true, Some("https://s/blob.png")),
            summary("👍", 1, false, None),
        ];
        let out = to_fedibird_reactions(&reactions, D, true);
        assert_eq!(
            out,
            vec![
                FedibirdReaction {
                    name: "blob".to_owned(),
                    count: 3,
                    me: Some(true),
                    url: Some("https://m/blob.png".to_owned()),
                    static_url: Some("https://m/blob.png".to_owned()),
                    domain: Some("misskey.example".to_owned()),
                },
                FedibirdReaction {
                    name: "👍".to_owned(),
                    count: 1,
                    me: Some(false),
                    url: None,
                    static_url: None,
                    domain: None,
                },
            ]
        );
        assert!(to_fedibird_reactions(&reactions, D, false)
            .iter()
            .all(|r| r.me.is_none()));
    }

    #[test]
    fn request_name_becomes_reaction_content() {
        assert_eq!(content_from_name("👍"), "👍");
        assert_eq!(content_from_name("blob"), ":blob:");
        assert_eq!(
            content_from_name("blob@misskey.example"),
            ":blob@misskey.example:"
        );
        assert_eq!(content_from_name(":blob:"), ":blob:");
        assert_eq!(bare_name("blob@misskey.example"), "blob");
        assert_eq!(bare_name(":blob:"), "blob");
        assert_eq!(bare_name("👍"), "👍");
    }

    #[test]
    fn stream_update_reports_current_and_removed_emoji() {
        let body = NoteUpdatedBody {
            post_id: "10".to_owned(),
            reactions: vec![
                ReactionCount {
                    emoji: "🎉".to_owned(),
                    count: 2,
                    emoji_url: None,
                },
                ReactionCount {
                    emoji: "❤️".to_owned(),
                    count: 1,
                    emoji_url: None,
                },
            ],
            reactor_actor_id: Some("7".to_owned()),
            reactor_emoji: Some("🎉".to_owned()),
            reactor_previous_emoji: Some(":blob@.:".to_owned()),
        };
        let updates = reaction_updates(&body, 7, Some("🎉"), D);
        assert_eq!(updates.len(), 2);
        assert_eq!(
            (
                updates[0].name.as_str(),
                updates[0].count,
                updates[0].account_ids.clone()
            ),
            ("🎉", 2, vec!["7".to_owned()])
        );
        assert_eq!(
            (
                updates[1].name.as_str(),
                updates[1].count,
                updates[1].account_ids.len()
            ),
            ("blob", 0, 0)
        );
        assert_eq!(updates[1].status_id, "10");
    }
}
