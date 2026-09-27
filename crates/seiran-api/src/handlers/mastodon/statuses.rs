//! ステータス（投稿）の取得・作成・削除と、お気に入り・リポスト。
//!
//! 書き込み系はカスタム API のハンドラ（`create_note`・`create_reaction`・`delete_repost`・
//! `delete_note`）をそのまま呼んで検証・配送・通知を共有し、成功後に対象を読み直して
//! Mastodon の `Status` で返す（Mastodon 本家も操作後の最新状態の `Status` を返す）。

use std::collections::HashSet;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use crate::error::ApiError;
use crate::handlers::notes::dto::{CreateNoteRequest, PollCreateRequest};
use crate::handlers::notes::ReactRequest;
use crate::middleware::{AuthedUser, MaybeAuthedUser};
use crate::AppState;

use super::convert::{
    build_accounts_ordered, build_statuses, find_status, from_mastodon_visibility, scan_mention,
    FAVOURITE_REACTION,
};
use super::extract::{lenient, MastodonParams, MastodonQuery, PageParams};
use super::types::{MastodonAccount, MastodonContext, MastodonStatus};

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

pub(super) fn parse_id(id: &str) -> Result<i64, ApiError> {
    id.parse()
        .map_err(|_| ApiError::NotFound("RECORD_NOT_FOUND"))
}

/// 既存ハンドラの成功応答（`NoteResponse` 等の JSON）から `id` を取り出す。失敗応答はそのまま
/// 呼び出し元へ返す。
async fn created_id(resp: Response) -> Result<i64, Response> {
    if !resp.status().is_success() {
        return Err(resp);
    }
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .map_err(|e| internal(e).into_response())?;
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v["id"].as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| internal("作成応答に id がありません").into_response())
}

/// GET /api/v1/statuses/:id
pub async fn show(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MastodonStatus>, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    Ok(Json(find_status(&state, parse_id(&id)?, viewer).await?))
}

/// 祖先をたどる上限（Mastodon 本家は無制限だが、1リクエストの問い合わせ回数を抑える）。
const MAX_ANCESTORS: usize = 40;

/// GET /api/v1/statuses/:id/context
/// 祖先は返信先を1件ずつたどり（古い順）、子孫は `thread_descendants`（カスタム API の返信
/// タブと共通）のうち返信でつながるものだけを古い順に返す（引用は返信ツリーに含めない）。
pub async fn context(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MastodonContext>, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    let post_id = parse_id(&id)?;
    let target = state
        .posts
        .find_by_id_for_viewer(post_id, viewer)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))?;

    let mut ancestors = Vec::new();
    let mut next = target.reply_to_post_id;
    while let Some(parent_id) = next {
        if ancestors.len() >= MAX_ANCESTORS {
            break;
        }
        let Some(parent) = state
            .posts
            .find_by_id_for_viewer(parent_id, viewer)
            .await
            .map_err(internal)?
        else {
            break;
        };
        next = parent.reply_to_post_id;
        ancestors.push(parent);
    }
    ancestors.reverse();

    let mut descendants = state
        .posts
        .thread_descendants(post_id, 200, viewer)
        .await
        .map_err(internal)?;
    descendants.sort_unstable_by_key(|p| p.id);
    let mut in_thread: HashSet<i64> = HashSet::from([post_id]);
    descendants.retain(|p| match p.reply_to_post_id {
        Some(parent) if in_thread.contains(&parent) => {
            in_thread.insert(p.id);
            true
        }
        _ => false,
    });

    let (ancestors, descendants) = tokio::join!(
        build_statuses(&state, ancestors, viewer),
        build_statuses(&state, descendants, viewer),
    );
    Ok(Json(MastodonContext {
        ancestors: ancestors?,
        descendants: descendants?,
    }))
}

#[derive(Deserialize, Default)]
pub struct PollParams {
    #[serde(default, deserialize_with = "lenient::vec_string")]
    pub options: Vec<String>,
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub expires_in: Option<i64>,
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub multiple: Option<bool>,
}

#[derive(Deserialize, Default)]
pub struct CreateStatusParams {
    pub status: Option<String>,
    #[serde(default, deserialize_with = "lenient::vec_string")]
    pub media_ids: Vec<String>,
    pub poll: Option<PollParams>,
    #[serde(default, deserialize_with = "lenient::opt_id")]
    pub in_reply_to_id: Option<String>,
    /// Mastodon 4.5 の引用。Fedibird・Pleroma 系クライアントの `quote_id` も受ける。
    #[serde(default, alias = "quote_id", deserialize_with = "lenient::opt_id")]
    pub quoted_status_id: Option<String>,
    pub spoiler_text: Option<String>,
    pub visibility: Option<String>,
    pub language: Option<String>,
}

/// `direct` の宛先は Mastodon では本文中のメンションで決まる。seiran の DM は宛先アクターIDの
/// 明示を要求するため、本文の `@user`・`@user@host`・`@handle.tld` を既知のアクターに引き当てる。
async fn resolve_direct_recipients(state: &AppState, text: &str) -> Result<Vec<String>, ApiError> {
    let chars: Vec<char> = text.chars().collect();
    let mut handles = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@' {
            if let Some((handle, end)) = scan_mention(&chars, i) {
                handles.push(handle);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    let mut ids = Vec::new();
    for handle in handles {
        let (username, domain) = match handle.split_once('@') {
            Some((u, d)) if d.eq_ignore_ascii_case(state.local_domain.as_str()) => {
                (u.to_owned(), state.local_domain.to_string())
            }
            Some((u, d)) => (u.to_owned(), d.to_owned()),
            // `.` を含む単独ハンドルは Bsky ハンドル（`actors.domain` が空）。
            None if handle.contains('.') => (handle.clone(), String::new()),
            None => (handle.clone(), state.local_domain.to_string()),
        };
        if let Some(actor) = state
            .actors
            .find_by_username_domain(&username, &domain)
            .await
            .map_err(internal)?
        {
            let id = actor.id.to_string();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    if ids.is_empty() {
        return Err(ApiError::BadRequest(
            "DIRECT_REQUIRES_KNOWN_MENTION".to_owned(),
        ));
    }
    Ok(ids)
}

/// Mastodon の投稿パラメータを seiran の `CreateNoteRequest` にする。
async fn to_create_request(
    state: &AppState,
    params: CreateStatusParams,
) -> Result<CreateNoteRequest, ApiError> {
    let visibility = match params.visibility.as_deref() {
        None | Some("") => "public",
        Some(v) => from_mastodon_visibility(v)
            .ok_or_else(|| ApiError::BadRequest("INVALID_VISIBILITY".to_owned()))?,
    };
    let text = params.status.unwrap_or_default();
    let recipient_actor_ids = if visibility == "direct" {
        Some(resolve_direct_recipients(state, &text).await?)
    } else {
        None
    };
    let poll = params
        .poll
        .filter(|p| !p.options.is_empty())
        .map(|p| PollCreateRequest {
            choices: p.options,
            multiple: p.multiple,
            expires_at: None,
            expires_in_seconds: p.expires_in,
            expires_at_epoch_ms: None,
        });
    Ok(CreateNoteRequest {
        text: Some(text),
        attachment_ids: (!params.media_ids.is_empty()).then_some(params.media_ids),
        deliver_to_fedi: None,
        deliver_to_bsky: None,
        renote_id: None,
        reply_to_id: params.in_reply_to_id,
        quote_of_id: params.quoted_status_id,
        visibility: Some(visibility.to_owned()),
        recipient_actor_ids,
        bsky_embed_choice: None,
        poll,
        content_warning: params.spoiler_text.filter(|s| !s.trim().is_empty()),
        link_card_urls: Vec::new(),
        // seiran が扱わない言語（Mastodon は ISO 639 全般を送ってくる）は付けずに投稿する。
        language: params
            .language
            .filter(|l| seiran_common::is_supported_language(l)),
    })
}

/// POST /api/v1/statuses — 通常投稿・返信（`in_reply_to_id`）・引用（`quoted_status_id`）。
/// 添付の閲覧注意（`sensitive`）は seiran のローカル投稿では指定できないため無視する
/// （CW を付けたい場合は `spoiler_text`）。予約投稿（`scheduled_at`）は未対応。
pub async fn create(
    user: AuthedUser,
    State(state): State<AppState>,
    MastodonParams(params): MastodonParams<CreateStatusParams>,
) -> Response {
    let viewer = Some(user.actor_id);
    let req = match to_create_request(&state, params).await {
        Ok(req) => req,
        Err(e) => return e.into_response(),
    };
    let resp = crate::handlers::notes::create_note(user, State(state.clone()), Json(req))
        .await
        .into_response();
    let id = match created_id(resp).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match find_status(&state, id, viewer).await {
        Ok(status) => Json(status).into_response(),
        Err(e) => e.into_response(),
    }
}

/// DELETE /api/v1/statuses/:id — 削除した投稿を、下書きへの復元用に `text`（元の本文）付きで返す。
pub async fn delete(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let viewer = Some(user.actor_id);
    let post_id = match parse_id(&id) {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let post = match state.posts.find_by_id_for_viewer(post_id, viewer).await {
        Ok(Some(post)) => post,
        Ok(None) => return ApiError::NotFound("RECORD_NOT_FOUND").into_response(),
        Err(e) => return internal(e).into_response(),
    };
    let body = post.body.clone();
    let status = match build_statuses(&state, vec![post], viewer).await {
        Ok(mut s) => s.pop(),
        Err(e) => return e.into_response(),
    };
    let resp = crate::handlers::notes::delete_note(Path(id), user, State(state))
        .await
        .into_response();
    if !resp.status().is_success() {
        return resp;
    }
    match status {
        Some(mut status) => {
            status.text = Some(body);
            Json(status).into_response()
        }
        None => Json(serde_json::json!({})).into_response(),
    }
}

/// POST /api/v1/statuses/:id/favourite — `❤️` リアクションを付ける（別の絵文字で反応済みなら
/// 置き換わる。seiran のリアクションは1投稿1人1つ）。
pub async fn favourite(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let viewer = Some(user.actor_id);
    let resp = crate::handlers::notes::create_reaction(
        Path(id.clone()),
        user,
        State(state.clone()),
        Json(ReactRequest {
            content: FAVOURITE_REACTION.to_owned(),
        }),
    )
    .await;
    if !resp.status().is_success() {
        return resp;
    }
    status_response(&state, &id, viewer).await
}

/// POST /api/v1/statuses/:id/unfavourite — `❤️` リアクションだけを外す。付けていなくても
/// Mastodon 本家同様に現在の `Status` を返す。
pub async fn unfavourite(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let viewer = Some(user.actor_id);
    let resp = crate::handlers::notes::reactions::remove_reaction(
        &state,
        &user,
        &id,
        Some(FAVOURITE_REACTION),
    )
    .await;
    if !resp.status().is_success() && resp.status() != axum::http::StatusCode::NOT_FOUND {
        return resp;
    }
    status_response(&state, &id, viewer).await
}

/// POST /api/v1/statuses/:id/reblog — リポストのラッパー投稿（`reblog` に元投稿）を返す。
pub async fn reblog(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let viewer = Some(user.actor_id);
    let req = CreateNoteRequest {
        renote_id: Some(id),
        ..empty_create_request()
    };
    let resp = crate::handlers::notes::create_note(user, State(state.clone()), Json(req))
        .await
        .into_response();
    let wrapper_id = match created_id(resp).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match find_status(&state, wrapper_id, viewer).await {
        Ok(status) => Json(status).into_response(),
        Err(e) => e.into_response(),
    }
}

/// POST /api/v1/statuses/:id/unreblog — リポストを取り消し、元投稿を返す。
pub async fn unreblog(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let viewer = Some(user.actor_id);
    let resp = crate::handlers::notes::delete_repost(Path(id.clone()), user, State(state.clone()))
        .await
        .into_response();
    if !resp.status().is_success() && resp.status() != axum::http::StatusCode::NOT_FOUND {
        return resp;
    }
    status_response(&state, &id, viewer).await
}

fn empty_create_request() -> CreateNoteRequest {
    CreateNoteRequest {
        text: None,
        attachment_ids: None,
        deliver_to_fedi: None,
        deliver_to_bsky: None,
        renote_id: None,
        reply_to_id: None,
        quote_of_id: None,
        visibility: None,
        recipient_actor_ids: None,
        bsky_embed_choice: None,
        poll: None,
        content_warning: None,
        link_card_urls: Vec::new(),
        language: None,
    }
}

async fn status_response(state: &AppState, id: &str, viewer: Option<i64>) -> Response {
    let result = match parse_id(id) {
        Ok(post_id) => find_status(state, post_id, viewer).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(status) => Json(status).into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/statuses/:id/reblogged_by — リポストしたアカウント（取り消し済みは除く）。
pub async fn reblogged_by(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    MastodonQuery(page): MastodonQuery<PageParams>,
) -> Result<Json<Vec<MastodonAccount>>, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    let post_id = parse_id(&id)?;
    state
        .posts
        .find_by_id_for_viewer(post_id, viewer)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))?;
    let limit = page.page(40, 80).limit;
    let entries = state
        .posts
        .reposts_of(post_id, limit)
        .await
        .map_err(internal)?;
    let mut ids: Vec<i64> = Vec::new();
    for e in entries.into_iter().filter(|e| e.deleted_at.is_none()) {
        if !ids.contains(&e.actor_id) {
            ids.push(e.actor_id);
        }
    }
    Ok(Json(build_accounts_ordered(&state, &ids).await?))
}

/// GET /api/v1/statuses/:id/favourited_by — `❤️` でリアクションしたアカウント。
pub async fn favourited_by(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    MastodonQuery(page): MastodonQuery<PageParams>,
) -> Result<Json<Vec<MastodonAccount>>, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    let post_id = parse_id(&id)?;
    state
        .posts
        .find_by_id_for_viewer(post_id, viewer)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))?;
    let limit = page.page(40, 80).limit;
    let actors = state
        .reactions
        .actors_for_reaction(post_id, FAVOURITE_REACTION, viewer, limit)
        .await
        .map_err(internal)?;
    let ids: Vec<i64> = actors.iter().map(|a| a.id).collect();
    Ok(Json(build_accounts_ordered(&state, &ids).await?))
}

/// POST /api/v1/statuses/:id/pin — カスタム API の `pin_note`（Fedi の featured・Bsky の
/// `pinnedPost` 反映込み）に委譲する。
pub async fn pin(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let viewer = Some(user.actor_id);
    let resp = crate::handlers::notes::pin_note(Path(id.clone()), user, State(state.clone()))
        .await
        .into_response();
    if !resp.status().is_success() {
        return resp;
    }
    status_response(&state, &id, viewer).await
}

/// POST /api/v1/statuses/:id/unpin — ピン留めしていなくても現在の `Status` を返す。
pub async fn unpin(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let viewer = Some(user.actor_id);
    let resp = crate::handlers::notes::unpin_note(Path(id.clone()), user, State(state.clone()))
        .await
        .into_response();
    if !resp.status().is_success() && resp.status() != axum::http::StatusCode::NOT_FOUND {
        return resp;
    }
    status_response(&state, &id, viewer).await
}

/// POST /api/v1/statuses/:id/bookmark — 自分だけの保存（相手への通知・配送は無い）。
/// 見えない投稿はブックマークできない。
pub async fn bookmark(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MastodonStatus>, ApiError> {
    let post_id = parse_id(&id)?;
    find_status(&state, post_id, Some(user.actor_id)).await?;
    seiran_common::repository::bookmark::insert(
        &state.db,
        seiran_common::generate_snowflake_id(chrono::Utc::now()),
        user.actor_id,
        post_id,
    )
    .await
    .map_err(internal)?;
    Ok(Json(
        find_status(&state, post_id, Some(user.actor_id)).await?,
    ))
}

/// POST /api/v1/statuses/:id/unbookmark
pub async fn unbookmark(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MastodonStatus>, ApiError> {
    let post_id = parse_id(&id)?;
    seiran_common::repository::bookmark::delete(&state.db, user.actor_id, post_id)
        .await
        .map_err(internal)?;
    Ok(Json(
        find_status(&state, post_id, Some(user.actor_id)).await?,
    ))
}

/// GET /api/v1/bookmarks — ブックマークした順（新しい順）。カーソルはブックマーク ID。
/// 削除済み・見えなくなった投稿は除く。
pub async fn bookmarks(
    user: AuthedUser,
    State(state): State<AppState>,
    uri: axum::extract::OriginalUri,
    MastodonQuery(page): MastodonQuery<PageParams>,
) -> Result<Response, ApiError> {
    let viewer = Some(user.actor_id);
    let rows =
        seiran_common::repository::bookmark::list(&state.db, user.actor_id, page.page(20, 40))
            .await
            .map_err(internal)?;
    let cursors = match (rows.first(), rows.last()) {
        (Some(first), Some(last)) => Some((first.0, last.0)),
        _ => None,
    };
    let post_ids: Vec<i64> = rows.iter().map(|(_, post_id)| *post_id).collect();
    let mut posts =
        seiran_common::repository::find_visible_posts_by_ids(&state.db, &post_ids, viewer)
            .await
            .map_err(internal)?;
    posts.sort_by_key(|p| post_ids.iter().position(|id| *id == p.id));
    let statuses = build_statuses(&state, posts, viewer).await?;
    Ok(super::extract::paginated(
        &uri,
        &state.local_domain,
        statuses,
        cursors,
    ))
}

/// GET /api/v1/polls/:id — 投票の ID は投稿 ID と同じ。
pub async fn poll(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<super::types::MastodonPoll>, ApiError> {
    let viewer = me.map(|u| u.actor_id);
    find_status(&state, parse_id(&id)?, viewer)
        .await?
        .poll
        .map(Json)
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))
}

#[derive(Deserialize, Default)]
pub struct PollVoteParams {
    #[serde(default, deserialize_with = "lenient::vec_string")]
    pub choices: Vec<String>,
}

/// POST /api/v1/polls/:id/votes — カスタム API の `vote_poll`（AP の投票送信込み）に委譲する。
pub async fn poll_vote(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    MastodonParams(params): MastodonParams<PollVoteParams>,
) -> Response {
    let viewer = Some(user.actor_id);
    let Ok(option_indexes) = params
        .choices
        .iter()
        .map(|c| c.parse::<usize>())
        .collect::<Result<Vec<_>, _>>()
    else {
        return ApiError::BadRequest("INVALID_CHOICE".to_owned()).into_response();
    };
    let resp = crate::handlers::notes::poll::vote_poll(
        Path(id.clone()),
        user,
        State(state.clone()),
        Json(crate::handlers::notes::poll::PollVoteRequest { option_indexes }),
    )
    .await
    .into_response();
    if !resp.status().is_success() {
        return resp;
    }
    let result = match parse_id(&id) {
        Ok(post_id) => find_status(&state, post_id, viewer).await,
        Err(e) => Err(e),
    };
    match result.map(|s| s.poll) {
        Ok(Some(poll)) => Json(poll).into_response(),
        Ok(None) => ApiError::NotFound("RECORD_NOT_FOUND").into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_params_accept_form_style_nested_poll() {
        let map = super::super::extract::pairs_to_value([
            ("status", "hello"),
            ("visibility", "private"),
            ("media_ids[]", "10"),
            ("poll[options][]", "a"),
            ("poll[options][]", "b"),
            ("poll[expires_in]", "600"),
            ("poll[multiple]", "true"),
            ("quote_id", "42"),
        ]);
        let p: CreateStatusParams = serde_json::from_value(serde_json::Value::Object(map)).unwrap();
        assert_eq!(p.status.as_deref(), Some("hello"));
        assert_eq!(p.media_ids, vec!["10"]);
        let poll = p.poll.unwrap();
        assert_eq!(poll.options, vec!["a", "b"]);
        assert_eq!(poll.expires_in, Some(600));
        assert_eq!(poll.multiple, Some(true));
        assert_eq!(p.quoted_status_id.as_deref(), Some("42"));
    }

    #[test]
    fn create_params_accept_json_numbers_for_ids() {
        let p: CreateStatusParams = serde_json::from_value(serde_json::json!({
            "status": "x",
            "in_reply_to_id": 123,
            "quoted_status_id": "456",
        }))
        .unwrap();
        assert_eq!(p.in_reply_to_id.as_deref(), Some("123"));
        assert_eq!(p.quoted_status_id.as_deref(), Some("456"));
    }
}
