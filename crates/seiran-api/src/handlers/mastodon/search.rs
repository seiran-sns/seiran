//! GET /api/v2/search — アカウント・投稿・ハッシュタグの横断検索。

use axum::{extract::State, Json};
use serde::Deserialize;

use crate::error::ApiError;
use crate::handlers::open_target::{resolve_open_target, ResolvedTarget};
use crate::middleware::MaybeAuthedUser;
use crate::AppState;

use super::accounts::search_accounts;
use super::convert::{build_account, build_statuses, find_status};
use super::extract::{lenient, MastodonQuery, PageParams};
use super::timelines::to_tag;
use super::types::{MastodonSearchResults, MastodonStatus, MastodonTag};

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

#[derive(Deserialize, Default)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub resolve: Option<bool>,
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub offset: Option<i64>,
    #[serde(flatten)]
    pub page: PageParams,
}

fn is_url_like(q: &str) -> bool {
    q.starts_with("https://") || q.starts_with("http://") || q.starts_with("at://")
}

/// 投稿の全文検索。検索対象・回数制限はカスタム API（`GET /api/notes/search`）と同じ。
/// seiran の検索はオフセットではなく ID カーソルで続きを取るので、`offset` 指定（Mastodon
/// クライアントの「さらに読み込む」）には `max_id` 無しなら空を返して終端とする。
async fn search_statuses(
    state: &AppState,
    me: Option<&crate::middleware::AuthedUser>,
    q: &str,
    params: &SearchParams,
) -> Result<Vec<MastodonStatus>, ApiError> {
    let page = params.page.page(20, 40);
    if params.offset.unwrap_or(0) > 0 && page.until_id.is_none() {
        return Ok(Vec::new());
    }
    if let Some(user) = me {
        let is_initial_search = page.until_id.is_none() && page.since_id.is_none();
        crate::rate_limit::check_search_rate_limit(state, user.actor_id, is_initial_search).await?;
    }
    let viewer = me.map(|u| u.actor_id);
    let ids = crate::handlers::search::search_post_ids_by_cursor(
        state,
        q,
        page.limit as usize,
        page.until_id,
        page.since_id,
        me.map(|u| (u.actor_id, u.username.as_str())),
    )
    .await;
    let mut rows = seiran_common::repository::find_visible_posts_by_ids(&state.db, &ids, viewer)
        .await
        .map_err(internal)?;
    rows.sort_unstable_by_key(|p| std::cmp::Reverse(p.id));
    build_statuses(state, rows, viewer).await
}

fn search_hashtags(q: &str, local_domain: &str) -> Vec<MastodonTag> {
    let chars: Vec<char> = format!("#{}", q.trim().trim_start_matches('#'))
        .chars()
        .collect();
    match seiran_common::mention::scan_hashtag(&chars, 0) {
        Some((tag, end)) if end == chars.len() => vec![to_tag(&tag, local_domain)],
        _ => Vec::new(),
    }
}

/// GET /api/v2/search
pub async fn search(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    MastodonQuery(params): MastodonQuery<SearchParams>,
) -> Result<Json<MastodonSearchResults>, ApiError> {
    let q = params.q.trim().to_owned();
    let viewer = me.as_ref().map(|u| u.actor_id);
    let wants = |kind: &str| params.kind.as_deref().is_none_or(|k| k == kind);
    let resolve = params.resolve == Some(true) && me.is_some();
    let mut results = MastodonSearchResults {
        accounts: Vec::new(),
        statuses: Vec::new(),
        hashtags: Vec::new(),
    };
    if q.is_empty() {
        return Ok(Json(results));
    }

    // URL（AP ID・投稿 URL・AT URI）は「開く」機能と同じ解決で、その投稿/アカウントだけを返す。
    if is_url_like(&q) {
        if resolve {
            match resolve_open_target(&state, &q).await {
                Ok(ResolvedTarget::Post(id)) if wants("statuses") => {
                    results
                        .statuses
                        .push(find_status(&state, id, viewer).await?);
                }
                Ok(ResolvedTarget::Actor(actor)) if wants("accounts") => {
                    results.accounts.push(build_account(&state, &actor).await?);
                }
                _ => {}
            }
        }
        return Ok(Json(results));
    }

    let limit = params.page.page(20, 40).limit;
    if wants("accounts") && params.offset.unwrap_or(0) == 0 {
        results.accounts = search_accounts(&state, &q, limit, resolve).await?;
    }
    if wants("hashtags") && params.offset.unwrap_or(0) == 0 {
        results.hashtags = search_hashtags(&q, &state.local_domain);
    }
    if wants("statuses") {
        results.statuses = search_statuses(&state, me.as_ref(), &q, &params).await?;
    }
    Ok(Json(results))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashtag_search_accepts_single_tag_only() {
        assert_eq!(search_hashtags("#Rust", "d.example")[0].name, "rust");
        assert_eq!(search_hashtags("猫", "d.example")[0].name, "猫");
        assert!(search_hashtags("two words", "d.example").is_empty());
        assert!(search_hashtags("2026", "d.example").is_empty());
    }
}
