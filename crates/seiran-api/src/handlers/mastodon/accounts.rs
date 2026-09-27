//! アカウントの取得・検索・関係と、フォロー操作。

use axum::{
    extract::{OriginalUri, Path, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use seiran_common::repository::{Actor, FollowListRow};

use crate::error::ApiError;
use crate::handlers::follows::FollowTargetRequest;
use crate::handlers::open_target::{resolve_open_target, ResolvedTarget};
use crate::middleware::{AuthedUser, MaybeAuthedUser};
use crate::AppState;

use super::convert::{
    build_account, build_accounts_ordered, build_credential_account, build_relationships,
    build_statuses,
};
use super::extract::{id_cursors, lenient, paginated, MastodonQuery, PageParams};
use super::types::{MastodonAccount, MastodonRelationship};

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

fn parse_account_id(id: &str) -> Result<i64, ApiError> {
    id.parse()
        .map_err(|_| ApiError::NotFound("RECORD_NOT_FOUND"))
}

async fn find_actor(state: &AppState, id: &str) -> Result<Actor, ApiError> {
    state
        .actors
        .find_by_id(parse_account_id(id)?)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))
}

/// リモートアクターのプロフィールを表示時に再取得する（カスタム API・Misskey 互換 API と同じ）。
async fn refresh_if_remote(state: &AppState, actor: &Actor) {
    if actor.actor_type != "local" {
        state.enqueue_remote_profile_refresh(actor.id).await;
    }
}

/// GET /api/v1/accounts/verify_credentials
pub async fn verify_credentials(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<MastodonAccount>, ApiError> {
    let actor = find_actor(&state, &user.actor_id.to_string()).await?;
    Ok(Json(build_credential_account(&state, &actor).await?))
}

/// GET /api/v1/accounts/:id
pub async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MastodonAccount>, ApiError> {
    let actor = find_actor(&state, &id).await?;
    refresh_if_remote(&state, &actor).await;
    Ok(Json(build_account(&state, &actor).await?))
}

#[derive(Deserialize, Default)]
pub struct AccountStatusesParams {
    #[serde(flatten)]
    pub page: PageParams,
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub pinned: Option<bool>,
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub exclude_replies: Option<bool>,
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub exclude_reblogs: Option<bool>,
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub only_media: Option<bool>,
}

/// GET /api/v1/accounts/:id/statuses — プロフィールの投稿一覧（DM は含めない）。
/// `pinned=true` はピン留め投稿。`exclude_replies`・`exclude_reblogs`・`only_media` は取得した
/// ページを絞り込むだけなので、絞り込み後の件数は `limit` より少ないことがある（`Link` の
/// カーソルは絞り込み前の行で作るので、次ページの取得は途切れない）。
pub async fn statuses(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    Path(id): Path<String>,
    MastodonQuery(params): MastodonQuery<AccountStatusesParams>,
) -> Result<Response, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    let actor_id = parse_account_id(&id)?;
    if params.pinned == Some(true) {
        let rows = state
            .pinned_posts
            .list_timeline_by_actor(actor_id, viewer)
            .await
            .map_err(internal)?;
        return Ok(Json(build_statuses(&state, rows, viewer).await?).into_response());
    }
    let page = params.page.page(20, 40);
    let mut rows = state
        .posts
        .timeline_by_actor(
            actor_id,
            viewer,
            page.limit,
            page.until_id,
            page.since_id,
            true,
        )
        .await
        .map_err(internal)?;
    let cursors = id_cursors(
        rows.iter()
            .map(|p| p.id.to_string())
            .collect::<Vec<_>>()
            .iter()
            .map(String::as_str),
    );
    if params.exclude_replies == Some(true) {
        rows.retain(|p| p.reply_to_post_id.is_none() && p.reply_to_ap_uri.is_none());
    }
    if params.exclude_reblogs == Some(true) {
        rows.retain(|p| p.repost_of_post_id.is_none() && p.repost_of_ap_uri.is_none());
    }
    let mut statuses = build_statuses(&state, rows, viewer).await?;
    if params.only_media == Some(true) {
        statuses.retain(|s| !s.media_attachments.is_empty());
    }
    Ok(paginated(&uri, &state.local_domain, statuses, cursors))
}

#[derive(Clone, Copy)]
enum FollowDirection {
    Following,
    Followers,
}

/// `followers`・`following` 共通。カーソルはフォロー行の ID（Mastodon 本家も同じく
/// フォロー関係の ID で `Link` を組み、本文のアカウント ID とは別物）。
async fn follow_list(
    state: &AppState,
    viewer: Option<i64>,
    uri: &OriginalUri,
    id: &str,
    page: PageParams,
    direction: FollowDirection,
) -> Result<Response, ApiError> {
    let actor_id = parse_account_id(id)?;
    let page = page.page(40, 80);
    let rows: Vec<FollowListRow> = match direction {
        FollowDirection::Following => {
            state
                .follows
                .list_following(actor_id, viewer, page.limit, page.until_id, page.since_id)
                .await
        }
        FollowDirection::Followers => {
            state
                .follows
                .list_followers(actor_id, viewer, page.limit, page.until_id, page.since_id)
                .await
        }
    }
    .map_err(internal)?;
    let cursors = match (rows.first(), rows.last()) {
        (Some(first), Some(last)) => Some((first.follow_id, last.follow_id)),
        _ => None,
    };
    let ids: Vec<i64> = rows.iter().map(|r| r.actor_id).collect();
    let accounts = build_accounts_ordered(state, &ids).await?;
    Ok(paginated(uri, &state.local_domain, accounts, cursors))
}

/// GET /api/v1/accounts/:id/followers
pub async fn followers(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    Path(id): Path<String>,
    MastodonQuery(page): MastodonQuery<PageParams>,
) -> Result<Response, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    follow_list(&state, viewer, &uri, &id, page, FollowDirection::Followers).await
}

/// GET /api/v1/accounts/:id/following
pub async fn following(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    uri: OriginalUri,
    Path(id): Path<String>,
    MastodonQuery(page): MastodonQuery<PageParams>,
) -> Result<Response, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    follow_list(&state, viewer, &uri, &id, page, FollowDirection::Following).await
}

#[derive(Deserialize, Default)]
pub struct RelationshipsParams {
    #[serde(default, deserialize_with = "lenient::vec_string")]
    pub id: Vec<String>,
}

/// GET /api/v1/accounts/relationships?id[]=1&id[]=2
pub async fn relationships(
    user: AuthedUser,
    State(state): State<AppState>,
    MastodonQuery(params): MastodonQuery<RelationshipsParams>,
) -> Result<Json<Vec<MastodonRelationship>>, ApiError> {
    let ids: Vec<i64> = params.id.iter().filter_map(|s| s.parse().ok()).collect();
    Ok(Json(
        build_relationships(&state, user.actor_id, &ids).await?,
    ))
}

async fn relationship(
    state: &AppState,
    viewer: i64,
    id: i64,
) -> Result<MastodonRelationship, ApiError> {
    build_relationships(state, viewer, &[id])
        .await?
        .pop()
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))
}

/// `acct`（`user`・`user@host`・`@user@host`・Bsky ハンドル）を既知のアクターに引き当てる。
/// 知らないアクターは `resolve` が真のときだけ WebFinger 等で取り込む。
async fn lookup_acct(
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
            .map_err(internal)?
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

#[derive(Deserialize)]
pub struct LookupParams {
    pub acct: String,
}

/// GET /api/v1/accounts/lookup?acct=
pub async fn lookup(
    State(state): State<AppState>,
    MastodonQuery(params): MastodonQuery<LookupParams>,
) -> Result<Json<MastodonAccount>, ApiError> {
    let actor = lookup_acct(&state, &params.acct, true)
        .await?
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))?;
    refresh_if_remote(&state, &actor).await;
    Ok(Json(build_account(&state, &actor).await?))
}

#[derive(Deserialize, Default)]
pub struct AccountSearchParams {
    #[serde(default)]
    pub q: String,
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub limit: Option<i64>,
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub resolve: Option<bool>,
}

/// アカウント検索（`/api/v1/accounts/search` と `/api/v2/search` 共通）。表示名・各ハンドルの
/// 部分一致（カスタム API の `GET /api/actors/search` と同じ）。`resolve` かつ `q` が
/// `@user@host` 等として解決できれば、そのアクターを先頭に置く（未知なら取り込む）。
pub async fn search_accounts(
    state: &AppState,
    q: &str,
    limit: i64,
    resolve: bool,
) -> Result<Vec<MastodonAccount>, ApiError> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids: Vec<i64> = Vec::new();
    if q.starts_with('@') || q.contains('@') {
        if let Some(actor) = lookup_acct(state, q, resolve).await? {
            ids.push(actor.id);
        }
    }
    let pattern = format!(
        "%{}%",
        crate::handlers::actor_search::escape_like(q.trim_start_matches('@'))
    );
    let rows = seiran_common::repository::actor_search::search_contains(&state.db, &pattern, limit)
        .await
        .map_err(internal)?;
    for row in rows {
        if !ids.contains(&row.id) {
            ids.push(row.id);
        }
    }
    ids.truncate(limit as usize);
    build_accounts_ordered(state, &ids).await
}

/// GET /api/v1/accounts/search
pub async fn search(
    _user: AuthedUser,
    State(state): State<AppState>,
    MastodonQuery(params): MastodonQuery<AccountSearchParams>,
) -> Result<Json<Vec<MastodonAccount>>, ApiError> {
    let limit = params.limit.unwrap_or(40).clamp(1, 80);
    Ok(Json(
        search_accounts(&state, &params.q, limit, params.resolve == Some(true)).await?,
    ))
}

/// POST /api/v1/accounts/:id/follow — カスタム API のフォロー処理に委譲し、関係を返す
/// （承認制の相手なら `requested: true`）。
pub async fn follow(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let viewer = user.actor_id;
    let target = match parse_account_id(&id) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let resp = crate::handlers::follows::create_follow(
        user,
        State(state.clone()),
        Json(FollowTargetRequest {
            actor_id: Some(id),
            ..FollowTargetRequest::default()
        }),
    )
    .await
    .into_response();
    // 既にフォロー済み（Conflict）でも Mastodon 本家は現在の関係を返す。
    if !resp.status().is_success() && resp.status() != axum::http::StatusCode::CONFLICT {
        return resp;
    }
    match relationship(&state, viewer, target).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => e.into_response(),
    }
}

/// POST /api/v1/accounts/:id/unfollow
pub async fn unfollow(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let viewer = user.actor_id;
    let target = match parse_account_id(&id) {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let resp = crate::handlers::follows::delete_follow(
        user,
        State(state.clone()),
        Json(FollowTargetRequest {
            actor_id: Some(id),
            ..FollowTargetRequest::default()
        }),
    )
    .await
    .into_response();
    if !resp.status().is_success() && resp.status() != axum::http::StatusCode::NOT_FOUND {
        return resp;
    }
    match relationship(&state, viewer, target).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => e.into_response(),
    }
}

/// ブロック・ミュートの操作の種類。
#[derive(Clone, Copy)]
enum ModerationOp {
    Block,
    Unblock,
    Mute,
    Unmute,
}

/// ブロック・ミュートの共通手順。カスタム API と同じ処理（`blocks::block_actor` 等）を
/// 呼び、関係を返す（Mastodon 本家は既にその状態でも成功として関係を返す）。
async fn moderate(user: AuthedUser, state: AppState, id: String, op: ModerationOp) -> Response {
    let result = async {
        let target = find_actor(&state, &id).await?;
        match op {
            ModerationOp::Block => {
                crate::handlers::blocks::block_actor(&state, &user, &target).await?
            }
            ModerationOp::Unblock => {
                crate::handlers::blocks::unblock_actor(&state, &user, &target).await?
            }
            ModerationOp::Mute => {
                crate::handlers::mutes::mute_actor(&state, user.actor_id, target.id).await?
            }
            ModerationOp::Unmute => {
                crate::handlers::mutes::unmute_actor(&state, user.actor_id, target.id).await?
            }
        }
        relationship(&state, user.actor_id, target.id).await
    }
    .await;
    match result {
        Ok(r) => Json(r).into_response(),
        Err(e) => e.into_response(),
    }
}

/// POST /api/v1/accounts/:id/block
pub async fn block(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    moderate(user, state, id, ModerationOp::Block).await
}

/// POST /api/v1/accounts/:id/unblock
pub async fn unblock(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    moderate(user, state, id, ModerationOp::Unblock).await
}

/// POST /api/v1/accounts/:id/mute — `notifications`・`duration` は受け取っても使わない
/// （seiran のミュートは常に通知も含めて無期限）。
pub async fn mute(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    moderate(user, state, id, ModerationOp::Mute).await
}

/// POST /api/v1/accounts/:id/unmute
pub async fn unmute(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    moderate(user, state, id, ModerationOp::Unmute).await
}

/// GET /api/v1/blocks — ブロック中のアカウント（ページングせず全件）。
pub async fn blocks(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<MastodonAccount>>, ApiError> {
    let ids: Vec<i64> = state
        .blocks
        .list_blocked(user.actor_id)
        .await
        .map_err(internal)?
        .into_iter()
        .map(|r| r.id)
        .collect();
    Ok(Json(build_accounts_ordered(&state, &ids).await?))
}

/// GET /api/v1/mutes — ミュート中のアカウント（ページングせず全件）。
pub async fn mutes(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<MastodonAccount>>, ApiError> {
    let ids: Vec<i64> = state
        .mutes
        .list_muted(user.actor_id)
        .await
        .map_err(internal)?
        .into_iter()
        .map(|r| r.id)
        .collect();
    Ok(Json(build_accounts_ordered(&state, &ids).await?))
}

#[derive(Deserialize)]
pub struct FieldAttribute {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Deserialize, Default)]
pub struct UpdateCredentialsParams {
    pub display_name: Option<String>,
    pub note: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub locked: Option<bool>,
    /// フォームでは `fields_attributes[0][name]=...`（添字をキーにしたオブジェクト）、
    /// JSON では配列で送られてくる。
    pub fields_attributes: Option<serde_json::Value>,
}

/// `fields_attributes` を添字順の項目列にする（名前・値とも空の行は除く）。
fn field_attributes(value: serde_json::Value) -> Vec<FieldAttribute> {
    let items: Vec<(i64, serde_json::Value)> = match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(i, v)| (i as i64, v))
            .collect(),
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| (k.parse().unwrap_or(i64::MAX), v))
            .collect(),
        _ => Vec::new(),
    };
    let mut items: Vec<(i64, FieldAttribute)> = items
        .into_iter()
        .filter_map(|(i, v)| {
            serde_json::from_value::<FieldAttribute>(v)
                .ok()
                .map(|f| (i, f))
        })
        .filter(|(_, f)| !(f.name.trim().is_empty() && f.value.trim().is_empty()))
        .collect();
    items.sort_by_key(|(i, _)| *i);
    items.into_iter().map(|(_, f)| f).collect()
}

/// multipart の画像ファイルをアバター・バナー用に保存し、メディア ID を返す（カスタム API の
/// アップロードと同じ `store_uploaded_file`）。
async fn store_profile_image(
    state: &AppState,
    user: &AuthedUser,
    part: super::extract::UploadedPart,
    media_type: &str,
) -> Result<String, ApiError> {
    let auth = crate::middleware::auth::AuthUser {
        user_id: user.user_id,
        email: user.email.clone(),
    };
    let Json(file) = crate::handlers::drive::store_uploaded_file(
        state,
        &auth,
        crate::handlers::drive::UploadedFile {
            bytes: part.bytes,
            media_type,
            deliver_to_bsky: true,
            original_filename: part.file_name,
        },
    )
    .await?;
    Ok(file.id)
}

/// PATCH /api/v1/accounts/update_credentials — 表示名・自己紹介・アバター・ヘッダー・
/// プロフィール項目・フォロー承認制を更新する。更新自体はカスタム API の
/// `update_profile`・`update_lock` に委譲する（AP `Update`・ATP プロフィールコミットも同じ）。
/// `bot`・`discoverable`・`source[...]` 等 seiran に無い項目は無視する。
pub async fn update_credentials(
    user: AuthedUser,
    headers: axum::http::HeaderMap,
    State(state): State<AppState>,
    mut form: super::extract::MastodonForm,
) -> Response {
    let result = async {
        let params: UpdateCredentialsParams = form.deserialize()?;
        let avatar_media_id = match form.files.remove("avatar") {
            Some(part) => Some(Some(
                store_profile_image(&state, &user, part, "avatar").await?,
            )),
            None => None,
        };
        let banner_media_id = match form.files.remove("header") {
            Some(part) => Some(Some(
                store_profile_image(&state, &user, part, "banner").await?,
            )),
            None => None,
        };
        let req = crate::handlers::users::UpdateProfileRequest {
            display_name: params.display_name,
            bio: params.note,
            avatar_media_id,
            banner_media_id,
            profile_fields: params.fields_attributes.map(|v| {
                field_attributes(v)
                    .into_iter()
                    .map(|f| crate::handlers::users::ProfileField {
                        name: f.name,
                        value: f.value,
                    })
                    .collect()
            }),
            birthday: None,
            birthday_public: None,
        };
        let resp = crate::handlers::users::update_profile(headers, State(state.clone()), Json(req))
            .await
            .into_response();
        if !resp.status().is_success() {
            return Ok(Some(resp));
        }
        if let Some(is_locked) = params.locked {
            let Json(_) = crate::handlers::account::update_lock(
                user.clone(),
                State(state.clone()),
                Json(crate::handlers::account::UpdateLockRequest { is_locked }),
            )
            .await?;
        }
        Ok::<_, ApiError>(None)
    }
    .await;
    match result {
        Ok(Some(error_resp)) => error_resp,
        Ok(None) => match find_actor(&state, &user.actor_id.to_string()).await {
            Ok(actor) => match build_credential_account(&state, &actor).await {
                Ok(a) => Json(a).into_response(),
                Err(e) => e.into_response(),
            },
            Err(e) => e.into_response(),
        },
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_attributes_accept_indexed_object_and_array() {
        let form = super::super::extract::pairs_to_value([
            ("fields_attributes[1][name]", "b"),
            ("fields_attributes[1][value]", "2"),
            ("fields_attributes[0][name]", "a"),
            ("fields_attributes[0][value]", "1"),
            ("fields_attributes[2][name]", ""),
            ("fields_attributes[2][value]", ""),
        ]);
        let fields = field_attributes(form["fields_attributes"].clone());
        assert_eq!(
            fields
                .iter()
                .map(|f| (f.name.as_str(), f.value.as_str()))
                .collect::<Vec<_>>(),
            vec![("a", "1"), ("b", "2")]
        );
        let fields = field_attributes(serde_json::json!([{"name": "x", "value": "y"}]));
        assert_eq!(fields.len(), 1);
    }
}
