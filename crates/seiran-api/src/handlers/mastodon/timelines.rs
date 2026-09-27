//! タイムライン・ハッシュタグ・リスト。取得はカスタム API・Misskey 互換 API と同じリポジトリ
//! 関数（可視性・ミュート・ブロックの判定込み）を使い、Mastodon の `Status` 配列と `Link`
//! ヘッダーで返す。
//!
//! Mastodon のタイムラインは DM（`direct`）を含めない（DM は会話 API で見る）ため、
//! `exclude_direct` は常に `true`。

use axum::{
    extract::{OriginalUri, Path, State},
    response::Response,
    Json,
};
use serde::Deserialize;

use seiran_common::repository::TimelinePost;

use crate::error::ApiError;
use crate::middleware::{AuthedUser, MaybeAuthedUser};
use crate::AppState;

use super::convert::build_statuses;
use super::extract::{id_cursors, lenient, paginated, MastodonQuery, PageParams};
use super::types::{MastodonList, MastodonTag};

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

/// Mastodon のタイムラインの既定件数・上限。
const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 40;

/// 取得した投稿を `Status` にし、`Link` ヘッダーを付ける。カーソルは変換前の行 ID で作る
/// （リポスト元が見えないリポスト等が変換で除かれても、次ページの起点がずれないように）。
async fn respond(
    state: &AppState,
    uri: &OriginalUri,
    rows: Vec<TimelinePost>,
    viewer: Option<i64>,
) -> Result<Response, ApiError> {
    let ids: Vec<String> = rows.iter().map(|p| p.id.to_string()).collect();
    let statuses = build_statuses(state, rows, viewer).await?;
    Ok(paginated(
        uri,
        &state.local_domain,
        statuses,
        id_cursors(ids.iter().map(String::as_str)),
    ))
}

#[derive(Deserialize, Default)]
pub struct TimelineParams {
    #[serde(flatten)]
    pub page: PageParams,
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub local: Option<bool>,
}

/// GET /api/v1/timelines/home
pub async fn home(
    user: AuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    MastodonQuery(params): MastodonQuery<TimelineParams>,
) -> Result<Response, ApiError> {
    let page = params.page.page(DEFAULT_LIMIT, MAX_LIMIT);
    let rows = state
        .posts
        .home_timeline(
            user.actor_id,
            page.limit,
            page.until_id,
            page.since_id,
            true,
        )
        .await
        .map_err(internal)?;
    respond(&state, &uri, rows, Some(user.actor_id)).await
}

/// GET /api/v1/timelines/public — `local=true` でローカル、それ以外は連合（seiran の
/// グローバルタイムライン）。`remote=true`（リモートのみ）は区別できないため連合と同じ。
pub async fn public(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    MastodonQuery(params): MastodonQuery<TimelineParams>,
) -> Result<Response, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    let page = params.page.page(DEFAULT_LIMIT, MAX_LIMIT);
    let rows = if params.local == Some(true) {
        state
            .posts
            .local_timeline(viewer, page.limit, page.until_id, page.since_id, true)
            .await
    } else {
        state
            .posts
            .global_timeline(viewer, page.limit, page.until_id, page.since_id, true)
            .await
    }
    .map_err(internal)?;
    respond(&state, &uri, rows, viewer).await
}

/// タグ名の正規化（`#` を除き小文字化。`hashtags` テーブルのキーと同じ形）。
fn normalize_tag(name: &str) -> String {
    name.trim().trim_start_matches('#').to_lowercase()
}

/// GET /api/v1/timelines/tag/:hashtag
pub async fn tag(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    Path(hashtag): Path<String>,
    MastodonQuery(params): MastodonQuery<TimelineParams>,
) -> Result<Response, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    let tag = normalize_tag(&hashtag);
    if tag.is_empty() {
        return Ok(paginated(&uri, &state.local_domain, Vec::<()>::new(), None));
    }
    let page = params.page.page(DEFAULT_LIMIT, MAX_LIMIT);
    let rows = state
        .hashtags
        .timeline(&tag, page.limit, page.until_id, page.since_id, viewer)
        .await
        .map_err(internal)?;
    respond(&state, &uri, rows, viewer).await
}

pub fn to_tag(name: &str, local_domain: &str) -> MastodonTag {
    let name = normalize_tag(name);
    MastodonTag {
        url: format!("https://{local_domain}/tags/{}", urlencoding::encode(&name)),
        name,
        history: Vec::new(),
        following: false,
    }
}

/// GET /api/v1/tags/:name — ハッシュタグの情報（フォロー機能は無いので常に未フォロー）。
pub async fn tag_info(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<MastodonTag>, ApiError> {
    if normalize_tag(&name).is_empty() {
        return Err(ApiError::NotFound("RECORD_NOT_FOUND"));
    }
    Ok(Json(to_tag(&name, &state.local_domain)))
}

fn to_list(row: seiran_common::repository::ListRow) -> MastodonList {
    MastodonList {
        id: row.id.to_string(),
        title: row.name,
        replies_policy: "list",
        exclusive: false,
    }
}

/// GET /api/v1/lists — 自分のリスト（非公開含む）。
pub async fn lists(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<MastodonList>>, ApiError> {
    let rows = state
        .lists
        .list_by_owner(user.actor_id)
        .await
        .map_err(internal)?;
    Ok(Json(rows.into_iter().map(to_list).collect()))
}

/// 閲覧可能なリスト（非公開リストは所有者本人のみ。カスタム API・Misskey 互換 API と同じ判定）。
async fn find_viewable_list(
    state: &AppState,
    id: &str,
    viewer: Option<i64>,
) -> Result<seiran_common::repository::ListRow, ApiError> {
    let list_id: i64 = id
        .parse()
        .map_err(|_| ApiError::NotFound("RECORD_NOT_FOUND"))?;
    let row = state
        .lists
        .find_by_id(list_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))?;
    if !row.is_public && viewer != Some(row.owner_actor_id) {
        return Err(ApiError::NotFound("RECORD_NOT_FOUND"));
    }
    Ok(row)
}

/// GET /api/v1/lists/:id
pub async fn list_show(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MastodonList>, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    Ok(Json(to_list(
        find_viewable_list(&state, &id, viewer).await?,
    )))
}

/// GET /api/v1/timelines/list/:id
pub async fn list(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    Path(id): Path<String>,
    MastodonQuery(params): MastodonQuery<TimelineParams>,
) -> Result<Response, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    let list = find_viewable_list(&state, &id, viewer).await?;
    let page = params.page.page(DEFAULT_LIMIT, MAX_LIMIT);
    let rows = state
        .lists
        .timeline(list.id, page.limit, page.until_id, page.since_id)
        .await
        .map_err(internal)?;
    respond(&state, &uri, rows, viewer).await
}
