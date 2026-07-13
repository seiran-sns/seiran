//! Misskey 実物のパス・POSTオンリー規約に合わせた**追加**エンドポイント。
//!
//! 書き込み系（リアクション作成/削除・リノート取消・フォロー作成/削除）は既存の
//! `handlers::notes`/`handlers::follows` の関数を直接呼び出して副作用（AP/ATP配送・
//! ストリーミング配信）ロジックを再利用し、成功時のレスポンスだけ Misskey 流
//! （`204 No Content`）に整形する。エラー時は既存の `ApiError` 形状をそのまま返す
//! （Misskey 本家のエラーID/種別は再現していない。将来の課題）。

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;

use crate::error::ApiError;
use crate::handlers::follows::{CreateFollowRequest, DeleteFollowRequest};
use crate::handlers::notes::ReactRequest;
use crate::middleware::extract_auth;
use crate::AppState;

use super::convert::{build_me_detailed, build_note, build_notes, build_user_detailed};
use super::types::{MisskeyMeDetailed, MisskeyNote, MisskeyUserDetailed};

// ─── リクエストDTO（Misskey 本家の camelCase フィールド名に合わせる） ──────────

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TimelineBody {
    pub limit: Option<i64>,
    pub since_id: Option<String>,
    pub until_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteIdBody {
    pub note_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionCreateBody {
    pub note_id: String,
    pub reaction: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UserShowBody {
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub host: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowingBody {
    pub user_id: String,
}

// ─── 共通ヘルパー ───────────────────────────────────────────────────────

/// ログイン済みなら actor_id を返し、未ログインなら `None`（読み取り系は匿名許可のため）。
async fn optional_actor_id(headers: &HeaderMap, state: &AppState) -> Option<i64> {
    let auth_user = extract_auth(headers, &state.local_auth).await.ok()?;
    state.actors.find_local_by_user_id(auth_user.user_id).await.ok().flatten().map(|a| a.id)
}

/// Misskey の `userId`（=seiran の actors.id）から、既存の follows.rs が期待する
/// 人間可読ターゲット文字列（ローカルusername / DID / AP URI）を逆算する。
async fn actor_id_to_target(state: &AppState, actor_id: i64) -> Result<String, ApiError> {
    let actor = state
        .actors
        .find_by_id(actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("USER_NOT_FOUND"))?;

    let target = if actor.domain == state.local_domain {
        actor.username.clone()
    } else if let Some(did) = &actor.at_did {
        did.clone()
    } else if let Some(uri) = &actor.ap_uri {
        uri.clone()
    } else {
        format!("{}@{}", actor.username, actor.domain)
    };
    Ok(target)
}

/// 既存ハンドラの成功レスポンスを Misskey 流の `204 No Content` に整形する。
/// エラー時（2xx以外）は既存の ApiError レスポンスをそのまま透過する。
fn as_no_content(resp: Response) -> Response {
    if resp.status().is_success() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        resp
    }
}

// ─── 自分自身・ユーザー ─────────────────────────────────────────────────

/// POST /api/i
pub async fn api_i(headers: HeaderMap, State(state): State<AppState>) -> Result<Json<MisskeyMeDetailed>, ApiError> {
    let auth_user = extract_auth(&headers, &state.local_auth).await?;
    let actor = state
        .actors
        .find_local_by_user_id(auth_user.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;
    Ok(Json(build_me_detailed(&state, &actor).await))
}

/// POST /api/users/show
pub async fn users_show(
    State(state): State<AppState>,
    Json(body): Json<UserShowBody>,
) -> Result<Json<MisskeyUserDetailed>, ApiError> {
    let actor = if let Some(uid) = body.user_id {
        let id: i64 = uid.parse().map_err(|_| ApiError::NotFound("USER_NOT_FOUND"))?;
        state.actors.find_by_id(id).await
    } else if let Some(username) = body.username {
        let domain = body.host.unwrap_or_else(|| state.local_domain.clone());
        state.actors.find_by_username_domain(&username, &domain).await
    } else {
        return Err(ApiError::BadRequest("USER_ID_OR_USERNAME_REQUIRED".to_owned()));
    }
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::NotFound("USER_NOT_FOUND"))?;

    Ok(Json(build_user_detailed(&state, &actor).await))
}

// ─── ノート ──────────────────────────────────────────────────────────

/// POST /api/notes/show
pub async fn notes_show(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<NoteIdBody>,
) -> Result<Json<MisskeyNote>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let post_id: i64 = body.note_id.parse().map_err(|_| ApiError::NotFound("NOTE_NOT_FOUND"))?;
    let post = state
        .posts
        .find_by_id(post_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOTE_NOT_FOUND"))?;
    Ok(Json(build_note(&state, post, my_actor_id).await))
}

/// POST /api/notes/local-timeline
pub async fn notes_local_timeline(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<TimelineBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let limit = body.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = body.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = body.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .posts
        .local_timeline(limit, until_id, since_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
}

/// POST /api/notes/timeline（ホームタイムライン。要ログイン）
pub async fn notes_home_timeline(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<TimelineBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let auth_user = extract_auth(&headers, &state.local_auth).await?;
    let actor_id = state
        .actors
        .find_local_by_user_id(auth_user.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?
        .id;

    let limit = body.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = body.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = body.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .posts
        .home_timeline(actor_id, limit, until_id, since_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(build_notes(&state, rows, Some(actor_id)).await))
}

/// POST /api/notes/reactions/create
pub async fn reactions_create(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ReactionCreateBody>,
) -> impl IntoResponse {
    let resp = crate::handlers::notes::create_reaction(
        Path(body.note_id),
        headers,
        State(state),
        Json(ReactRequest { content: body.reaction }),
    )
    .await
    .into_response();
    as_no_content(resp)
}

/// POST /api/notes/reactions/delete
/// Misskey は `noteId` のみを受け取る（1投稿1ユーザー1リアクションが前提のため対象の絵文字を
/// 指定する必要がない）。既存の `delete_reaction` は絵文字をパスパラメータに取るため、
/// ここで現在のリアクション内容を引いてから委譲する。
pub async fn reactions_delete(headers: HeaderMap, State(state): State<AppState>, Json(body): Json<NoteIdBody>) -> Response {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let actor_id = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a.id,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };
    let note_id: i64 = match body.note_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let content: Option<String> = sqlx::query_scalar("SELECT content FROM reactions WHERE post_id = $1 AND actor_id = $2")
        .bind(note_id)
        .bind(actor_id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    let content = match content {
        Some(c) => c,
        None => return ApiError::NotFound("NOT_REACTED").into_response(),
    };

    let resp = crate::handlers::notes::delete_reaction(Path((body.note_id, content)), headers, State(state))
        .await
        .into_response();
    as_no_content(resp)
}

/// POST /api/notes/unrenote
pub async fn notes_unrenote(headers: HeaderMap, State(state): State<AppState>, Json(body): Json<NoteIdBody>) -> impl IntoResponse {
    let resp = crate::handlers::notes::delete_repost(Path(body.note_id), headers, State(state))
        .await
        .into_response();
    as_no_content(resp)
}

// ─── フォロー ────────────────────────────────────────────────────────

/// POST /api/following/create
pub async fn following_create(headers: HeaderMap, State(state): State<AppState>, Json(body): Json<FollowingBody>) -> Response {
    // ターゲット解決（DB問い合わせ）より先に認証を確認する。未認証のまま先に解決すると
    // 「このIDのユーザーは存在するか」を匿名で探索できてしまう（列挙攻撃対策）。
    if let Err(e) = extract_auth(&headers, &state.local_auth).await {
        return e.into_response();
    }
    let actor_id: i64 = match body.user_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_USER_ID".to_owned()).into_response(),
    };
    let target = match actor_id_to_target(&state, actor_id).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let resp = crate::handlers::follows::create_follow(headers, State(state), Json(CreateFollowRequest { target }))
        .await
        .into_response();
    as_no_content(resp)
}

/// POST /api/following/delete
pub async fn following_delete(headers: HeaderMap, State(state): State<AppState>, Json(body): Json<FollowingBody>) -> Response {
    if let Err(e) = extract_auth(&headers, &state.local_auth).await {
        return e.into_response();
    }
    let actor_id: i64 = match body.user_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_USER_ID".to_owned()).into_response(),
    };
    let target = match actor_id_to_target(&state, actor_id).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let resp = crate::handlers::follows::delete_follow(headers, State(state), Json(DeleteFollowRequest { target }))
        .await
        .into_response();
    as_no_content(resp)
}
