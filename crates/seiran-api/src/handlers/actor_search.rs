//! アクターのサジェスト検索（リスト機能のメンバー追加入力補助等）。
//!
//! DB（`actors`テーブル）に既に存在するアクターのみを対象にした軽量な部分一致検索。
//! WebFinger解決等の新規リモート取得は行わない（あくまで「知っている」アクターの絞り込み）。

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;

use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct ActorSearchQuery {
    pub q: String,
    pub limit: Option<i64>,
}

use seiran_common::repository::actor_search::{self, ActorOrigin, HostFilter, UserSearchFilter};
use seiran_common::repository::Actor;

use crate::error::ApiError;
use crate::handlers::open_target::{resolve_open_target, ResolvedTarget};

/// `GET /api/actors/search?q=...&limit=...`
/// ユーザー名・表示名の部分一致でDB上のアクターを検索する（リスト機能のメンバー追加
/// サジェスト用）。list-relay等の`users`行を持たないシステムアクターは除外する。
pub async fn search_actors(
    _user: AuthedUser,
    Query(q): Query<ActorSearchQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // リスト編集・DM宛先向け。先頭の`@`を含む生入力を、表示名・Fedi/Bsky/ローカルの
    // 各ハンドルを改行で区切って連結した検索用文字列へ部分一致させる。
    let query = q.q.trim();
    if query.is_empty() {
        return Json(Vec::<serde_json::Value>::new()).into_response();
    }
    let limit = q.limit.unwrap_or(10).clamp(1, 30);
    let contains_pattern = format!("%{}%", escape_like(query));

    let rows = actor_search::search_contains(&state.db, &contains_pattern, limit).await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[actor_search] DBエラー: {}", e);
            return Json(Vec::<serde_json::Value>::new()).into_response();
        }
    };

    let out: Vec<serde_json::Value> = rows.into_iter().map(suggestion_json).collect();

    Json(out).into_response()
}

/// アクター候補1件の応答（`api.actors.search` の `ActorSuggestion`）。
fn suggestion_json(row: actor_search::ActorSearchRow) -> serde_json::Value {
    let actor_search::ActorSearchRow {
        id,
        username,
        domain,
        display_name,
        emoji_map,
        actor_type,
        avatar_url,
    } = row;
    // add_member/also-known-as にそのまま渡せるターゲット文字列を計算する。
    // `resolve_and_upsert_target` は先頭の`@`を無条件で除去するため無くても
    // 動作はするが、ハンドルの慣習的な表記（`@user`/`@user@domain`）に揃える。
    let target = seiran_common::username::actor_handle(&username, &domain, &actor_type);
    let avatar_url =
        seiran_common::avatar::resolve_avatar_url(avatar_url, &actor_type, &domain, id);
    serde_json::json!({
        "actor_id": id.to_string(),
        "username": username,
        "domain": domain,
        "display_name": display_name,
        "emojis": seiran_common::emoji_map::to_string_map(emoji_map.as_ref()),
        "actor_type": actor_type,
        "avatar_url": avatar_url,
        "target": target,
    })
}

#[derive(Deserialize)]
pub struct UserSearchQuery {
    pub q: String,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// `GET /api/users/search?q=...&limit=...&offset=...` — 検索画面の「ユーザー」タブ。
/// 検索は Misskey 互換の `users/search`・Mastodon 互換の `accounts/search` と同じ
/// `search_user_ids`（同じパスの `POST` が Misskey 互換）。応答は `api.actors.search` と同じ形。
/// 回数制限はポストの検索と同じ枠で数える（続きの読み込みは数えない）。
pub async fn search_users(
    user: AuthedUser,
    Query(q): Query<UserSearchQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    if q.q.trim().is_empty() {
        return Ok(Json(Vec::new()));
    }
    let offset = q.offset.unwrap_or(0);
    crate::rate_limit::check_search_rate_limit(&state, user.actor_id, offset == 0).await?;
    let search = UserSearch {
        query: &q.q,
        origin: ActorOrigin::Any,
        viewer: Some(user.actor_id),
        following_only: false,
        resolve: true,
        limit: q.limit.unwrap_or(30).clamp(1, 100),
        offset,
    };
    let ids = search_user_ids(&state, &search).await?;
    let rows = actor_search::find_rows_by_ids(&state.db, &ids)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(rows.into_iter().map(suggestion_json).collect()))
}

/// `GET /api/actors/suggest?q=...&limit=...`
/// 投稿欄の`@`入力補助向け。`q`は先頭の`@`を除いた文字列で、ハンドルに前方一致させる。
pub async fn suggest_actors(
    _user: AuthedUser,
    Query(q): Query<ActorSearchQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let query = q.q.trim().trim_start_matches('@');
    if query.is_empty() {
        return Json(Vec::<serde_json::Value>::new()).into_response();
    }
    let limit = q.limit.unwrap_or(10).clamp(1, 30);
    let pattern = format!("{}%", escape_like(query));
    let local_bsky_pattern = format!("\n@{}%", escape_like(query));

    let rows =
        actor_search::suggest_by_handle_prefix(&state.db, &pattern, &local_bsky_pattern, limit)
            .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[actor_suggest] DBエラー: {}", e);
            return Json(Vec::<serde_json::Value>::new()).into_response();
        }
    };

    let query_lower = query.to_lowercase();
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(
            |actor_search::ActorSearchRow {
                 id,
                 username,
                 domain,
                 display_name,
                 emoji_map,
                 actor_type,
                 avatar_url,
             }| {
                let target = suggestion_target(&actor_type, &username, &domain, &query_lower);
                let avatar_url =
                    seiran_common::avatar::resolve_avatar_url(avatar_url, &actor_type, &domain, id);
                serde_json::json!({
                    "actor_id": id.to_string(),
                    "username": username,
                    "domain": domain,
                    "display_name": display_name,
                    "emojis": seiran_common::emoji_map::to_string_map(emoji_map.as_ref()),
                    "actor_type": actor_type,
                    "avatar_url": avatar_url,
                    "target": target,
                })
            },
        )
        .collect();

    Json(out).into_response()
}

fn suggestion_target(actor_type: &str, username: &str, domain: &str, query_lower: &str) -> String {
    if actor_type != "local" {
        return if domain.is_empty() {
            username.to_owned()
        } else {
            format!("{}@{}", username, domain)
        };
    }

    let username_lower = username.to_lowercase();
    if username_lower.starts_with(query_lower) {
        username.to_owned()
    } else if format!("{}@", username_lower).starts_with(query_lower) {
        format!("{}@{}", username, domain)
    } else {
        format!("{}.{}", username_lower, domain)
    }
}

/// `ILIKE` パターン中の `%`/`_`/`\` をエスケープする（ユーザー入力をそのままワイルドカードに
/// しないため）。
pub(crate) fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// `acct`（`user`・`user@host`・`@user@host`・Bsky ハンドル）を既知のアクターに引き当てる。
/// 知らないアクターは `resolve` が真のときだけ WebFinger 等で取り込む。
pub async fn lookup_acct(
    state: &AppState,
    acct: &str,
    resolve: bool,
) -> Result<Option<Actor>, ApiError> {
    let acct = acct.trim().trim_start_matches('@');
    if acct.is_empty() {
        return Ok(None);
    }
    let local_domain = state.local_domain.as_str();
    let candidates: Vec<(String, String)> = match acct.split_once('@') {
        Some((u, d)) if d.eq_ignore_ascii_case(local_domain) => {
            vec![(u.to_owned(), local_domain.to_owned())]
        }
        Some((u, d)) => vec![(u.to_owned(), d.to_owned())],
        // `.` を含む単独ハンドルは Bsky ハンドル（`actors.domain` が空）の可能性もある。
        None if acct.contains('.') => vec![
            (acct.to_owned(), String::new()),
            (acct.to_owned(), local_domain.to_owned()),
        ],
        None => vec![(acct.to_owned(), local_domain.to_owned())],
    };
    for (username, domain) in candidates {
        if let Some(actor) = state
            .actors
            .find_by_username_domain(&username, &domain)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
        {
            return Ok(Some(actor));
        }
    }
    if !resolve {
        return Ok(None);
    }
    match resolve_open_target(state, &format!("@{acct}")).await {
        Ok(ResolvedTarget::Actor(actor)) => Ok(Some(*actor)),
        _ => Ok(None),
    }
}

// ─── ユーザー検索（Misskey 互換・Mastodon 互換で共通） ─────────────────────

/// ユーザー検索の条件。Misskey の `users/search` と Mastodon の `accounts/search`・
/// `v2/search` が同じ関数を通し、差は応答の整形だけにする。
pub struct UserSearch<'a> {
    pub query: &'a str,
    pub origin: ActorOrigin,
    pub viewer: Option<i64>,
    /// 閲覧者がフォローしている相手だけにする（Mastodon の `following`）。
    pub following_only: bool,
    /// 完全一致のハンドルが未知のとき WebFinger 等で取り込む（Mastodon の `resolve`）。
    /// 未ログインでは取り込まない。
    pub resolve: bool,
    pub limit: i64,
    pub offset: i64,
}

/// ユーザー検索の ID 一覧（表示順）。表示名・各ハンドル表記・自己紹介の部分一致を、
/// 表示名かハンドルに一致した人を先にしてユーザー名順に並べる（2文字以上のときだけ）。`query` が `user@host`・`@user`・Bsky ハンドル等として既知のアクター（`resolve`
/// なら取り込んだアクター）に引き当たれば、最初のページの先頭に置き、以降のページには
/// 出さない（部分一致の並びに埋もれると目当ての相手が見つからないため）。
/// ブロック・ミュートの相手、凍結・退会済みは出さない。未ログインではローカルのうち
/// ログアウト中の閲覧者に見せてよいアクターだけを出す。
pub async fn search_user_ids(state: &AppState, s: &UserSearch<'_>) -> Result<Vec<i64>, ApiError> {
    let query = s.query.trim();
    if query.is_empty() || s.limit <= 0 {
        return Ok(Vec::new());
    }
    let escaped = escape_like(query.trim_start_matches('@'));
    let pattern = format!("%{escaped}%");
    let prefix = format!("{escaped}%");
    let mut filter = UserSearchFilter {
        contains_pattern: &pattern,
        prefix_pattern: &prefix,
        origin: s.origin,
        viewer: s.viewer,
        following_only: s.following_only && s.viewer.is_some(),
        exclude_id: None,
        limit: s.limit,
        offset: s.offset.max(0),
    };
    let internal = |e: sqlx::Error| ApiError::Internal(e.to_string());

    let exact = if looks_like_acct(query) {
        let resolve = s.resolve && s.viewer.is_some() && s.offset <= 0;
        match lookup_acct(state, query, resolve).await? {
            Some(actor) => {
                actor_search::filter_searchable_user_ids(&state.db, &filter, &[actor.id])
                    .await
                    .map_err(internal)?
                    .first()
                    .copied()
            }
            None => None,
        }
    } else {
        None
    };
    filter.exclude_id = exact;

    let mut ids = Vec::new();
    if let Some(id) = exact {
        if filter.offset == 0 {
            ids.push(id);
            filter.limit -= 1;
        } else {
            // 最初のページで1件分を使ったので、部分一致の位置を1つ戻す。
            filter.offset -= 1;
        }
    }
    if filter.limit > 0 && query.trim_start_matches('@').chars().count() >= MIN_PARTIAL_MATCH_CHARS
    {
        ids.extend(
            actor_search::search_user_ids(&state.db, &filter)
                .await
                .map_err(internal)?,
        );
    }
    Ok(ids)
}

/// 部分一致で探す最短の文字数。1文字はほぼすべてのアクターに一致し、索引を使っても
/// 並べ替えに秒単位かかる（入力のたびに検索するクライアントが最初の1文字で投げてくる）。
/// 1文字のハンドル・名前の相手は、完全一致の引き当てで見つかる。
const MIN_PARTIAL_MATCH_CHARS: usize = 2;

/// ハンドルとして引き当てを試す入力か（空白を含まない）。`@` もドメインも無い語は
/// ローカルのユーザー名として引き当てる（完全一致の相手を先頭に出すため）。
fn looks_like_acct(q: &str) -> bool {
    !q.chars().any(char::is_whitespace)
}

/// Misskey の `users/search-by-username-and-host`（メンションの補完）。`host` が `None`
/// なら全サーバー、自ドメインか `.` ならローカル、それ以外はサーバー名の前方一致。
pub async fn search_user_ids_by_username_and_host(
    state: &AppState,
    viewer: Option<i64>,
    username: Option<&str>,
    host: Option<&str>,
    limit: i64,
) -> Result<Vec<i64>, ApiError> {
    let username_pattern = username
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(|u| format!("{}%", escape_like(u.trim_start_matches('@'))));
    let host_pattern;
    let host_filter = match host.map(str::trim).filter(|h| !h.is_empty()) {
        None => HostFilter::Any,
        Some(h) if h == "." || h.eq_ignore_ascii_case(&state.local_domain) => HostFilter::Local,
        Some(h) => {
            host_pattern = format!("{}%", escape_like(h));
            HostFilter::Prefix(&host_pattern)
        }
    };
    actor_search::search_user_ids_by_username_and_host(
        &state.db,
        viewer,
        username_pattern.as_deref(),
        host_filter,
        limit,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::suggestion_target;

    #[test]
    fn local_target_follows_matching_handle_form() {
        assert_eq!(
            suggestion_target("local", "Alice", "example.com", "ali"),
            "Alice"
        );
        assert_eq!(
            suggestion_target("local", "Alice", "example.com", "alice@"),
            "Alice@example.com"
        );
        assert_eq!(
            suggestion_target("local", "Alice", "example.com", "alice."),
            "alice.example.com"
        );
    }
}
