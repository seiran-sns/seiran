//! 検索エンドポイント（フェーズ6）
//!
//! GET /api/notes/search?q=<query>&limit=<n>&session_id=<id>&until_id=<id>&since_id=<id>
//!   &since=<RFC 3339>&until=<RFC 3339>&user_id=<id>&server=<server>&tag=<tag>
//!
//! ローカル DB と Bsky AppView（public.api.bsky.app）を並行検索してブレンドする。
//!
//! 検索はログイン必須（Misskey・Mastodon 互換 API も同じ）。全文検索はクエリによって重く、
//! AppView の結果の保存（外部への問い合わせを伴う）も起きるので、回数制限の掛からない
//! 未ログインのリクエストには開かない。共通関数は閲覧者を必須の引数に取る。

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use seiran_common::atp::AppViewSearchOptions;
use seiran_common::repository::post_search::{PostSearchFilters, ServerFilter};

use super::notes::NoteResponse;
use crate::error::ApiError;
use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
    /// ページネーション継続用セッション ID（2ページ目以降に付与）
    pub session_id: Option<String>,
    /// このIDより古い投稿を取得する（Misskey互換の追加検索）。
    #[serde(alias = "untilId")]
    pub until_id: Option<i64>,
    /// このIDより新しい投稿を取得する。AppViewには問い合わせない。
    #[serde(alias = "sinceId")]
    pub since_id: Option<i64>,
    /// この時刻以降に作られた投稿（RFC 3339）。
    pub since: Option<String>,
    /// この時刻より前に作られた投稿（RFC 3339）。
    pub until: Option<String>,
    /// 投稿者のアクター ID。
    pub user_id: Option<String>,
    /// 投稿者のサーバー（`server_filter_from` の書式）。
    pub server: Option<String>,
    /// ハッシュタグ（先頭 `#` は有っても無くてもよい）。
    pub tag: Option<String>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub notes: Vec<NoteResponse>,
    pub session_id: Option<String>,
}

/// 投稿検索の要求（検索式と、検索式とは別に指定する条件）。
pub(crate) struct PostSearch<'a> {
    pub query: &'a str,
    pub filters: PostSearchFilters,
}

impl PostSearch<'_> {
    fn is_empty(&self) -> bool {
        self.query.trim().is_empty() && self.filters.is_empty()
    }
}

/// サーバーの指定を `ServerFilter` にする。`local`（Misskey 互換の `.`・自ドメインも同じ）・
/// `fediverse`・`bluesky`・それ以外はホスト名。空なら絞らない。
pub(crate) fn server_filter_from(value: Option<&str>, local_domain: &str) -> ServerFilter {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return ServerFilter::Any;
    };
    let host = value.to_lowercase();
    match host.as_str() {
        "local" | "." => ServerFilter::Local,
        "fediverse" => ServerFilter::Fediverse,
        "bluesky" => ServerFilter::Bluesky,
        h if h.eq_ignore_ascii_case(local_domain) => ServerFilter::Local,
        _ => ServerFilter::Host(host),
    }
}

/// ハッシュタグの指定を `hashtags.name` と同じ形（先頭 `#` なし・小文字）にする。
pub(crate) fn normalize_hashtag(value: Option<&str>) -> Option<String> {
    value
        .map(|v| v.trim().trim_start_matches('#').to_lowercase())
        .filter(|v| !v.is_empty())
}

fn parse_datetime(value: Option<&str>) -> Result<Option<DateTime<Utc>>, ApiError> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| {
            DateTime::parse_from_rfc3339(v)
                .map(|t| t.with_timezone(&Utc))
                .map_err(|_| ApiError::BadRequest("INVALID_DATE".to_owned()))
        })
        .transpose()
}

fn filters_from_query(q: &SearchQuery, local_domain: &str) -> Result<PostSearchFilters, ApiError> {
    let actor_id = q
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|id| {
            id.parse::<i64>()
                .map_err(|_| ApiError::BadRequest("INVALID_USER_ID".to_owned()))
        })
        .transpose()?;
    Ok(PostSearchFilters {
        since: parse_datetime(q.since.as_deref())?,
        until: parse_datetime(q.until.as_deref())?,
        actor_id,
        server: server_filter_from(q.server.as_deref(), local_domain),
        hashtag: normalize_hashtag(q.tag.as_deref()),
    })
}

pub async fn search_notes(
    Query(q): Query<SearchQuery>,
    State(state): State<AppState>,
    user: AuthedUser,
) -> impl IntoResponse {
    let raw_query = q.q.as_deref().unwrap_or("").trim().to_string();
    let filters = match filters_from_query(&q, &state.local_domain) {
        Ok(f) => f,
        Err(e) => return e.into_response(),
    };
    let search = PostSearch {
        query: &raw_query,
        filters,
    };
    if search.is_empty() {
        return Json(SearchResponse {
            notes: vec![],
            session_id: None,
        })
        .into_response();
    }

    // スクロールによる続き取得（session_id/until_id/since_idのいずれかを指定）は
    // レート制限の回数に含めない（issue #223）。
    let is_initial_search = q.session_id.is_none() && q.until_id.is_none() && q.since_id.is_none();
    if let Err(e) =
        crate::rate_limit::check_search_rate_limit(&state, user.actor_id, is_initial_search).await
    {
        return e.into_response();
    }

    let limit = q.limit.unwrap_or(30).clamp(1, 100) as usize;
    let viewer = (user.actor_id, user.username.as_str());

    // since/until指定（Misskey互換のカーソルページング）はセッションを使わない。
    if q.since_id.is_some() || q.until_id.is_some() {
        let ids =
            search_post_ids_by_cursor(&state, &search, limit, q.until_id, q.since_id, viewer).await;
        return fetch_and_respond(&state, ids, None, Some(viewer.0)).await;
    }

    // ── セッション継続（過去掘り） ────────────────────────────────────────────
    if let Some(ref sid) = q.session_id {
        if let Some((mut buf, local_until_id, appview_cursor)) = state.search_store.take_buffer(sid)
        {
            // バッファが十分あればそのまま返す
            if buf.len() >= limit {
                let ids: Vec<i64> = buf.drain(..limit).collect();
                state
                    .search_store
                    .put_buffer(sid, buf, local_until_id, appview_cursor);
                return fetch_and_respond(&state, ids, Some(sid.clone()), Some(viewer.0)).await;
            }

            // バッファ不足: ローカル DB と、続きがあれば AppView を追加フェッチ
            let appview = match appview_cursor {
                Some(cursor) => appview_options(&state, &search)
                    .await
                    .map(|options| (options, cursor)),
                None => None,
            };
            let (mut extra_local, (mut av_ids, next_cursor)) = tokio::join!(
                search_local_db(
                    &state.db,
                    &search,
                    limit as i64,
                    local_until_id,
                    None,
                    viewer
                ),
                async {
                    match &appview {
                        Some((options, cursor)) => {
                            search_appview(&state, &search, options, Some(cursor), limit).await
                        }
                        None => (Vec::new(), None),
                    }
                },
            );
            let new_local_until = extra_local.last().copied().or(local_until_id);
            buf.append(&mut extra_local);
            buf.append(&mut av_ids);

            // ソート・重複除去・ページング分割
            let buf = resolve_bridge_ids_for_search(&state.db, buf).await;
            let (ids, remaining) = merge_sort_dedup_and_split(buf, limit);
            state
                .search_store
                .put_buffer(sid, remaining, new_local_until, next_cursor);
            return fetch_and_respond(&state, ids, Some(sid.clone()), Some(viewer.0)).await;
        }
        // セッション消滅 → 新しい検索としてやり直す
    }

    // ── 初回リクエスト: ローカル DB + AppView 並行フェッチ ───────────────────
    let options = appview_options(&state, &search).await;
    let (local_ids, (mut av_ids, appview_cursor)) = tokio::join!(
        search_local_db(&state.db, &search, limit as i64, None, None, viewer),
        async {
            match &options {
                Some(options) => search_appview(&state, &search, options, None, limit).await,
                None => (Vec::new(), None),
            }
        },
    );
    let local_until_id = local_ids.last().copied();
    let mut all_ids = local_ids;
    all_ids.append(&mut av_ids);
    let all_ids = resolve_bridge_ids_for_search(&state.db, all_ids).await;

    let new_session_id = uuid::Uuid::new_v4().to_string();
    let (return_ids, remaining) = merge_sort_dedup_and_split(all_ids, limit);

    state.search_store.create(
        new_session_id.clone(),
        raw_query.clone(),
        remaining,
        local_until_id,
        appview_cursor,
    );

    fetch_and_respond(&state, return_ids, Some(new_session_id), Some(viewer.0)).await
}

/// 条件を AppView の `searchPosts` の絞り込みにする。Bluesky の投稿が一致しえない条件
/// （サーバーに Bluesky 以外を指定した、指定したユーザーが Bluesky に投稿していない）なら
/// `None`（問い合わせない）。
async fn appview_options(
    state: &AppState,
    search: &PostSearch<'_>,
) -> Option<AppViewSearchOptions> {
    let f = &search.filters;
    if !f.server.includes_bluesky() {
        return None;
    }
    let author = match f.actor_id {
        None => None,
        Some(id) => {
            // ローカルユーザーの Bluesky 側の投稿はローカル DB にある投稿そのものなので、
            // AppView には問い合わせない。
            let actor = state.actors.find_by_id(id).await.ok().flatten()?;
            if actor.actor_type == "local" {
                return None;
            }
            Some(actor.at_did?)
        }
    };
    Some(AppViewSearchOptions {
        since: f.since,
        until: f.until,
        author,
        tag: f.hashtag.clone(),
    })
}

/// AppView に問い合わせ、結果を取り込んだ post_id と次のカーソルを返す。
async fn search_appview(
    state: &AppState,
    search: &PostSearch<'_>,
    options: &AppViewSearchOptions,
    cursor: Option<&str>,
    limit: usize,
) -> (Vec<i64>, Option<String>) {
    // 検索語の AST を AppView の検索式に組み直す（語は引用符で囲み、`from:` 等を指定子として
    // 効かせない。`post_search::to_appview_query`）。何も一致しえない式なら問い合わせない。
    use seiran_common::repository::post_search::{parse, to_appview_query};
    let Some(text) = to_appview_query(&parse(search.query)) else {
        return (Vec::new(), None);
    };
    let (posts, next) =
        seiran_common::atp::search_appview_posts(&state.http_client, &text, cursor, limit, options)
            .await;
    let ids = persist_appview_posts(state, posts).await;
    // 投稿者・ハッシュタグ・日時は AppView が絞っているのでそのまま使う（DB 側の情報で
    // 確かめ直すと、ハッシュタグを記録していない取り込み済みの投稿が落ちる。AppView の
    // タグの照合は本文に出ない `tags` まで見るので、むしろ正確）。サーバーだけは AppView が
    // 絞れない（ローカル・リモート seiran のユーザーの Bluesky 側の投稿も返る）ので確かめる。
    let server_only = PostSearchFilters {
        server: search.filters.server.clone(),
        ..Default::default()
    };
    let ids =
        seiran_common::repository::post_search::filter_post_ids(&state.db, &ids, &server_only)
            .await
            .unwrap_or_default();
    (ids, next)
}

/// カーソル（`until_id`/`since_id`）指定の検索で、表示すべき post_id を新しい順に最大
/// `limit` 件返す。frontend API（`search_notes`）と Misskey 互換 API（`notes/search`）・
/// Mastodon 互換 API（`v2/search`）の共通実装。
/// - `since_id` 指定: ローカル DB のみ（逆方向ページング。要件どおりAppViewには問い合わせない）。
/// - それ以外: ローカル DB と AppView の双方から取得してブレンドする。`until_id` 指定時は
///   その投稿の作成時刻を AppView 側の上限にする。
pub(crate) async fn search_post_ids_by_cursor(
    state: &AppState,
    search: &PostSearch<'_>,
    limit: usize,
    until_id: Option<i64>,
    since_id: Option<i64>,
    viewer: (i64, &str),
) -> Vec<i64> {
    if since_id.is_some() {
        let ids = search_local_db(&state.db, search, limit as i64, None, since_id, viewer).await;
        return resolve_bridge_ids_for_search(&state.db, ids).await;
    }
    let until = match until_id {
        Some(id) => seiran_common::repository::post::created_at_of(&state.db, id)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let options = appview_options(state, search).await.map(|mut o| {
        o.until = match (o.until, until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        o
    });
    let (local_ids, (mut av_ids, _)) = tokio::join!(
        search_local_db(&state.db, search, limit as i64, until_id, None, viewer),
        async {
            match &options {
                Some(options) => search_appview(state, search, options, None, limit).await,
                None => (Vec::new(), None),
            }
        },
    );
    let mut all_ids = local_ids;
    all_ids.append(&mut av_ids);
    let all_ids = resolve_bridge_ids_for_search(&state.db, all_ids).await;
    merge_sort_dedup_and_split(all_ids, limit).0
}

/// ローカル DB・AppView 由来の post_id 列をマージして降順ソート・重複排除した上で、
/// 先頭 `limit` 件（今回返す分）と残り（次ページ用にセッションへバッファする分）に分割する。
///
/// ブレンドアルゴリズムの核心部分。`AppState`（DB・HTTPクライアント）に依存しない
/// 純粋関数として切り出すことで、DB・外部HTTPのセットアップなしに単体テスト可能にしている。
/// ブリッジポスト対応（`crate::bridge_post`・`docs/protocols.md`参照）: 検索結果の後処理として、
/// 未解決（元ポスト未取り込み）のブリッジポストは除外し、解決済みなら元ポストのidへ置換する。
/// 置換後に生じる重複は呼び出し元の`merge_sort_dedup_and_split`が吸収する。
async fn resolve_bridge_ids_for_search(db: &sqlx::PgPool, ids: Vec<i64>) -> Vec<i64> {
    if ids.is_empty() {
        return ids;
    }
    let rows = seiran_common::repository::note_extras::bridge_status_for_posts(db, &ids)
        .await
        .unwrap_or_default();
    let by_id: std::collections::HashMap<i64, (Option<i64>, bool)> = rows
        .into_iter()
        .map(|(id, bridge_of_post_id, is_bridge)| (id, (bridge_of_post_id, is_bridge)))
        .collect();
    ids.into_iter()
        .filter_map(|id| match by_id.get(&id) {
            Some((Some(original_id), _)) => Some(*original_id),
            Some((None, true)) => None,
            _ => Some(id),
        })
        .collect()
}

fn merge_sort_dedup_and_split(mut ids: Vec<i64>, limit: usize) -> (Vec<i64>, Vec<i64>) {
    ids.sort_by(|a, b| b.cmp(a));
    ids.dedup();
    let split_at = limit.min(ids.len());
    let return_ids: Vec<i64> = ids.drain(..split_at).collect();
    (return_ids, ids)
}

pub(crate) async fn search_local_db(
    db: &sqlx::PgPool,
    search: &PostSearch<'_>,
    fetch_limit: i64,
    until_id: Option<i64>,
    since_id: Option<i64>,
    me: (i64, &str),
) -> Vec<i64> {
    seiran_common::repository::post_search::search_local_post_ids(
        db,
        search.query,
        &search.filters,
        fetch_limit,
        until_id,
        since_id,
        Some(me),
    )
    .await
    .unwrap_or_default()
}

/// AppView検索結果のactor/postをローカルDBへupsertし、ローカルpost IDへ変換する。
pub(crate) async fn persist_appview_posts(
    state: &AppState,
    posts: Vec<seiran_common::atp::BskyPost>,
) -> Vec<i64> {
    let actor_ids = resolve_appview_authors(state, &posts).await;
    let mut ids = Vec::with_capacity(posts.len());
    for post in posts {
        let Some(&actor_id) = actor_ids.get(&post.author_did) else {
            continue;
        };
        // 返信許可・引用可否の取得は検索の応答を待たせないようジョブへ回す
        match seiran_common::atp::upsert_bsky_post_deferring_gates(
            &state.db,
            &state.job_queue,
            &state.http_client,
            actor_id,
            &post,
        )
        .await
        {
            Ok(id) => ids.push(id),
            Err(error) => {
                tracing::warn!("[search] AppView post保存失敗 uri={}: {}", post.uri, error)
            }
        }
    }
    ids
}

/// 同時に解決する投稿者の数（解決は DB の読み書きだけ）。
const AUTHOR_RESOLVE_CONCURRENCY: usize = 10;

/// AppView の検索結果の投稿者を DID ごとに1度だけ、並列に保存・解決する（DID → actor_id）。
/// 宣言レコードは取りに行かない: 投稿の `seiranPost.counterpartAuthorId` があればそれを申告に
/// 使い（`Known`）、無ければ後からジョブで取る（`Deferred`）。初見の投稿者は宣言の取得に
/// 1人あたり約1秒かかり、検索結果の投稿者は大半が初見なので、待つと1ページで数秒〜数十秒に
/// なる。解決できなかった投稿者は含めない。
async fn resolve_appview_authors(
    state: &AppState,
    posts: &[seiran_common::atp::BskyPost],
) -> std::collections::HashMap<String, i64> {
    use futures_util::stream::{self, StreamExt};

    // 投稿者ごとに、`seiranPost` の申告があればそれを持つ投稿を代表にする。
    let mut authors: std::collections::HashMap<&str, &seiran_common::atp::BskyPost> =
        std::collections::HashMap::new();
    for post in posts {
        authors
            .entry(post.author_did.as_str())
            .and_modify(|current| {
                if current.seiran_counterpart_author.is_none()
                    && post.seiran_counterpart_author.is_some()
                {
                    *current = post;
                }
            })
            .or_insert(post);
    }
    // future を先に作ってから流す（参照を取るクロージャを `map` に渡すと、axum のハンドラの
    // `Send` 判定でライフタイムが一般化できずコンパイルできない）。
    let lookups: Vec<_> = authors
        .into_values()
        .map(|post| async move {
            resolve_appview_author(state, post)
                .await
                .map(|id| (post.author_did.clone(), id))
        })
        .collect();
    stream::iter(lookups)
        .buffer_unordered(AUTHOR_RESOLVE_CONCURRENCY)
        .filter_map(std::future::ready)
        .collect()
        .await
}

async fn resolve_appview_author(
    state: &AppState,
    post: &seiran_common::atp::BskyPost,
) -> Option<i64> {
    use seiran_common::seiran_actor_merge::DeclarationLookup;
    match state.actors.find_by_did(&post.author_did).await {
        Ok(Some(actor)) if actor.actor_type == "local" => Some(actor.id),
        Ok(_) => {
            let lookup = match post.seiran_counterpart_author.as_deref() {
                Some(uri) => DeclarationLookup::Known(uri),
                None => DeclarationLookup::Deferred,
            };
            match seiran_common::seiran_actor_merge::discover_bsky_profile(
                &state.db,
                state.job_queue.as_ref(),
                &post.into(),
                lookup,
                true,
            )
            .await
            {
                Ok(outcome) => Some(outcome.actor_id),
                Err(error) => {
                    tracing::warn!(
                        "[search] AppView actor保存失敗 did={}: {}",
                        post.author_did,
                        error
                    );
                    None
                }
            }
        }
        Err(error) => {
            tracing::warn!(
                "[search] AppView actor検索失敗 did={}: {}",
                post.author_did,
                error
            );
            None
        }
    }
}

/// post_id リストからノートレスポンスを構築して返す。検索時点で可視性を判定済みの ID でも、
/// セッションにバッファした後で返す場合があるため、取得時に改めて閲覧者の可視性で絞る。
async fn fetch_and_respond(
    state: &AppState,
    ids: Vec<i64>,
    session_id: Option<String>,
    viewer_actor_id: Option<i64>,
) -> axum::response::Response {
    let mut rows =
        seiran_common::repository::find_visible_posts_by_ids(&state.db, &ids, viewer_actor_id)
            .await
            .unwrap_or_default();
    rows.sort_unstable_by_key(|p| std::cmp::Reverse(p.id));
    let notes = super::notes::build_note_responses(state, rows, viewer_actor_id).await;
    Json(SearchResponse { notes, session_id }).into_response()
}

#[cfg(test)]
mod tests {
    use super::{merge_sort_dedup_and_split, normalize_hashtag, server_filter_from, ServerFilter};

    #[test]
    fn server_filter_accepts_local_aliases_fediverse_bluesky_and_hosts() {
        let d = "seiran.example";
        assert_eq!(server_filter_from(None, d), ServerFilter::Any);
        assert_eq!(server_filter_from(Some(" "), d), ServerFilter::Any);
        assert_eq!(server_filter_from(Some("local"), d), ServerFilter::Local);
        assert_eq!(server_filter_from(Some("."), d), ServerFilter::Local);
        assert_eq!(
            server_filter_from(Some("Seiran.Example"), d),
            ServerFilter::Local
        );
        assert_eq!(
            server_filter_from(Some("fediverse"), d),
            ServerFilter::Fediverse
        );
        assert_eq!(
            server_filter_from(Some("bluesky"), d),
            ServerFilter::Bluesky
        );
        assert_eq!(
            server_filter_from(Some("Misskey.IO"), d),
            ServerFilter::Host("misskey.io".to_owned())
        );
    }

    #[test]
    fn hashtag_is_normalized_like_stored_names() {
        assert_eq!(
            normalize_hashtag(Some("#Seiran")),
            Some("seiran".to_owned())
        );
        assert_eq!(normalize_hashtag(Some("猫")), Some("猫".to_owned()));
        assert_eq!(normalize_hashtag(Some("#")), None);
        assert_eq!(normalize_hashtag(None), None);
    }

    /// 1ページ目: ローカル・Bsky合わせて limit 件を超える件数がある場合、
    /// 上位 limit 件が降順で返り、残りがバッファ用に残ること。
    #[test]
    fn first_page_returns_limit_and_buffers_the_rest() {
        let local_ids = vec![10, 8, 5, 2];
        let bsky_ids = vec![9, 6, 3, 1];
        let mut all_ids = local_ids;
        all_ids.extend(bsky_ids);

        let (returned, buffered) = merge_sort_dedup_and_split(all_ids, 3);

        assert_eq!(returned, vec![10, 9, 8]);
        assert_eq!(buffered, vec![6, 5, 3, 2, 1]);
    }

    /// 2ページ目: 1ページ目でバッファに残った分から、追加フェッチなしでも
    /// 続きが正しく降順で取り出せること（バッファ消費のみのケース）。
    #[test]
    fn second_page_consumes_buffer_from_previous_page() {
        // 1ページ目の結果としてバッファに残った状態を模す。
        let buffered_from_page1 = vec![6, 5, 3, 2, 1];

        let (returned, buffered) = merge_sort_dedup_and_split(buffered_from_page1, 3);

        assert_eq!(returned, vec![6, 5, 3]);
        assert_eq!(buffered, vec![2, 1]);
    }

    /// 3ページ目: バッファの残数が limit 未満でも、ある分だけ全て返し、
    /// バッファは空になること（末尾ページの挙動）。
    #[test]
    fn last_page_returns_remaining_items_and_empties_buffer() {
        let buffered_from_page2 = vec![2, 1];

        let (returned, buffered) = merge_sort_dedup_and_split(buffered_from_page2, 3);

        assert_eq!(returned, vec![2, 1]);
        assert!(buffered.is_empty());
    }

    /// ローカルDBとBsky AppViewの両方から同一投稿（同じローカルpost_id）が
    /// 見つかった場合、重複が排除されて1件のみ返ること。
    #[test]
    fn duplicate_ids_from_local_and_bsky_are_deduplicated() {
        let local_ids = vec![10, 7, 5];
        // AppView経由でローカルDBへマッピングした結果、ローカル検索と同じpost_idが混在。
        let bsky_ids = vec![10, 7, 3];
        let mut all_ids = local_ids;
        all_ids.extend(bsky_ids);

        let (returned, buffered) = merge_sort_dedup_and_split(all_ids, 10);

        assert_eq!(returned, vec![10, 7, 5, 3]);
        assert!(buffered.is_empty());
    }

    /// 空の入力（該当する投稿が1件もない）場合、両方とも空になること。
    #[test]
    fn empty_input_returns_empty_results() {
        let (returned, buffered) = merge_sort_dedup_and_split(vec![], 30);

        assert!(returned.is_empty());
        assert!(buffered.is_empty());
    }

    /// 全件が limit 以下に収まる場合、バッファは空になり全件がそのまま返ること。
    #[test]
    fn fewer_items_than_limit_returns_all_with_empty_buffer() {
        let (returned, buffered) = merge_sort_dedup_and_split(vec![5, 3, 1], 30);

        assert_eq!(returned, vec![5, 3, 1]);
        assert!(buffered.is_empty());
    }
}
