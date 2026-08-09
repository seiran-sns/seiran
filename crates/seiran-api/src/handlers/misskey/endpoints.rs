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

/// POST /api/endpoints
///
/// Misskeyクライアントが利用可能なAPIを機能検出するための一覧。
/// Ariaはここに`emojis`がある場合だけ`POST /api/emojis`を呼ぶ。
pub async fn endpoints() -> Json<Vec<&'static str>> {
    Json(vec![
        "drive/files/create",
        "emojis",
        "following/create",
        "following/delete",
        "i",
        "i/notifications",
        "meta",
        "notes/create",
        "notes/global-timeline",
        "notes/hybrid-timeline",
        "notes/local-timeline",
        "notes/reactions",
        "notes/reactions/create",
        "notes/reactions/delete",
        "notes/search",
        "notes/search-by-tag",
        "notes/show",
        "notes/timeline",
        "notes/unrenote",
        "users/followers",
        "users/following",
        "users/notes",
        "users/show",
    ])
}
use std::collections::HashMap;

use serde::Deserialize;

use seiran_common::repository::Actor;

use crate::error::ApiError;
use crate::handlers::follows::{CreateFollowRequest, DeleteFollowRequest};
use crate::handlers::notes::ReactRequest;
use crate::middleware::{extract_auth, AuthedUser};
use crate::AppState;

use super::convert::{
    build_me_detailed, build_note, build_notes, build_notifications, build_user_detailed,
    build_users_detailed, user_lite,
};
use super::types::{
    MisskeyFollowRelation, MisskeyMeDetailed, MisskeyNote, MisskeyNoteReaction,
    MisskeyNotification, MisskeyUserDetailed,
};

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
pub struct NotesSearchBody {
    pub query: String,
    pub limit: Option<i64>,
    pub until_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesSearchByTagBody {
    pub tag: String,
    pub limit: Option<i64>,
    pub since_id: Option<String>,
    pub until_id: Option<String>,
}

/// POST /api/notes/search-by-tag（Misskey互換、Ariaのハッシュタグ画面）。
pub async fn notes_search_by_tag(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<NotesSearchByTagBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let tag = body.tag.trim().trim_start_matches('#').to_lowercase();
    if tag.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let my_actor_id = optional_actor_id(&headers, &state).await;
    let limit = body.limit.unwrap_or(30).clamp(1, 100);
    let until_id = body.until_id.as_deref().and_then(|id| id.parse().ok());
    let since_id = body.since_id.as_deref().and_then(|id| id.parse().ok());
    let rows = state
        .hashtags
        .timeline(&tag, limit, until_id, since_id, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(build_notes(&state, rows, my_actor_id).await))
}

/// POST /api/notes/search（Misskey互換、Aria等）
pub async fn notes_search(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<NotesSearchBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let query = body.query.trim();
    if query.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let limit = body.limit.unwrap_or(30).clamp(1, 100);
    let until_id = body.until_id.as_deref().and_then(|id| id.parse().ok());
    let viewer_name = if let Some(actor_id) = my_actor_id {
        state
            .actors
            .find_by_id(actor_id)
            .await
            .ok()
            .flatten()
            .map(|actor| actor.username)
    } else {
        None
    };
    let until = if let Some(id) = until_id {
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT created_at FROM posts WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
    } else {
        None
    };
    let (local_ids, (appview_posts, _)) = tokio::join!(
        crate::handlers::search::search_local_db(
            &state.db,
            query,
            limit,
            until_id,
            None,
            my_actor_id.zip(viewer_name.as_deref()),
        ),
        seiran_common::atp::search_appview_posts(
            &state.http_client,
            query,
            None,
            limit as usize,
            until,
        ),
    );
    let mut ids = local_ids;
    ids.append(&mut crate::handlers::search::persist_appview_posts(&state, appview_posts).await);
    ids.sort_unstable_by(|a, b| b.cmp(a));
    ids.dedup();
    ids.truncate(limit as usize);

    let mut rows = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(post) = state
            .posts
            .find_by_id_for_viewer(id, my_actor_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
        {
            rows.push(post);
        }
    }
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
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

/// Misskey はローカルのカスタム絵文字を `:shortcode@.:` 形式で送る。
/// seiran の内部表現は `:shortcode:` なので、Misskey 互換 API の境界で変換する。
fn normalize_local_reaction(reaction: &str) -> String {
    reaction
        .strip_prefix(':')
        .and_then(|value| value.strip_suffix("@.:"))
        .map_or_else(|| reaction.to_owned(), |shortcode| format!(":{shortcode}:"))
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
pub struct UsersNotesBody {
    pub user_id: String,
    pub limit: Option<i64>,
    pub since_id: Option<String>,
    pub until_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowingBody {
    pub user_id: String,
}

/// `POST /api/users/following`・`POST /api/users/followers` 共通のリクエストボディ（#81）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRelationBody {
    pub user_id: String,
    pub limit: Option<i64>,
    pub since_id: Option<String>,
    pub until_id: Option<String>,
}

/// `POST /api/notes/reactions` のリクエストボディ（#81）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesReactionsBody {
    pub note_id: String,
    /// 本家 Misskey は省略可能（全種別対象）だが、seiran側の集計実装は単一絵文字指定が
    /// 前提のため、省略時は空配列を返す（`notes_reactions` 内のコメント参照）。
    #[serde(rename = "type")]
    pub reaction_type: Option<String>,
    pub limit: Option<i64>,
}

fn default_true() -> bool {
    true
}

/// `POST /api/i/notifications` のリクエストボディ。Misskey 本家の paramDef に合わせる
/// （`sinceDate`/`untilDate` は seiran では未対応、`sinceId`/`untilId` のみ）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationsBody {
    pub limit: Option<i64>,
    pub since_id: Option<String>,
    pub until_id: Option<String>,
    #[serde(default = "default_true")]
    pub mark_as_read: bool,
    pub include_types: Option<Vec<String>>,
    pub exclude_types: Option<Vec<String>>,
}

// ─── 共通ヘルパー ───────────────────────────────────────────────────────

/// ログイン済みなら actor_id を返し、未ログインなら `None`（読み取り系は匿名許可のため）。
async fn optional_actor_id(headers: &HeaderMap, state: &AppState) -> Option<i64> {
    let auth_user = extract_auth(
        headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    .ok()?;
    state
        .actors
        .find_local_by_user_id(auth_user.user_id)
        .await
        .ok()
        .flatten()
        .map(|a| a.id)
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

    let target = if actor.actor_type == "local" {
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
pub async fn api_i(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<MisskeyMeDetailed>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
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
        let id: i64 = uid
            .parse()
            .map_err(|_| ApiError::NotFound("USER_NOT_FOUND"))?;
        state.actors.find_by_id(id).await
    } else if let Some(username) = body.username {
        let domain = body.host.unwrap_or_else(|| state.local_domain.to_string());
        state
            .actors
            .find_by_username_domain(&username, &domain)
            .await
    } else {
        return Err(ApiError::BadRequest(
            "USER_ID_OR_USERNAME_REQUIRED".to_owned(),
        ));
    }
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::NotFound("USER_NOT_FOUND"))?;

    Ok(Json(build_user_detailed(&state, &actor).await))
}

/// POST /api/users/notes — プロフィール画面のノートタブ（Aria等）。
/// カスタムAPI `GET /api/users/posts`（`handlers::users::user_posts`）と同じ
/// `timeline_by_actor` を使うが、`exclude_direct=true` はカスタムAPI側の
/// `build_profile_response` 初回取得と同じ扱い（DMをプロフィール投稿一覧に含めない）。
pub async fn users_notes(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<UsersNotesBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let actor_id: i64 = body
        .user_id
        .parse()
        .map_err(|_| ApiError::NotFound("USER_NOT_FOUND"))?;
    let limit = body.limit.unwrap_or(10).clamp(1, 100);
    let until_id: Option<i64> = body.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = body.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .posts
        .timeline_by_actor(actor_id, my_actor_id, limit, until_id, since_id, true)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
}

// ─── ノート ──────────────────────────────────────────────────────────

/// POST /api/notes/show
pub async fn notes_show(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<NoteIdBody>,
) -> Result<Json<MisskeyNote>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let post_id: i64 = body
        .note_id
        .parse()
        .map_err(|_| ApiError::NotFound("NOTE_NOT_FOUND"))?;
    let post = state
        .posts
        .find_by_id_for_viewer(post_id, my_actor_id)
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

    // Misskey互換APIはMisskey本家の`specified`同様のデフォルト挙動を保つため、
    // `exclude_direct`は常に`false`（自分宛のdirectは含まれる）。
    let rows = state
        .posts
        .local_timeline(my_actor_id, limit, until_id, since_id, false)
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
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
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
        .home_timeline(actor_id, limit, until_id, since_id, false)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(build_notes(&state, rows, Some(actor_id)).await))
}

/// POST /api/notes/hybrid-timeline（ソーシャルタイムライン。要ログイン、#78）
pub async fn notes_hybrid_timeline(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<TimelineBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
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
        .social_timeline(actor_id, limit, until_id, since_id, false)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(build_notes(&state, rows, Some(actor_id)).await))
}

/// POST /api/notes/global-timeline（グローバルタイムライン、#78）
pub async fn notes_global_timeline(
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
        .global_timeline(my_actor_id, limit, until_id, since_id, false)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
}

/// POST /api/notes/reactions/create
pub async fn reactions_create(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ReactionCreateBody>,
) -> impl IntoResponse {
    let user = match crate::middleware::AuthedUser::from_headers(&headers, &state).await {
        Ok(u) => u,
        Err(e) => return as_no_content(e),
    };
    let resp = crate::handlers::notes::create_reaction(
        Path(body.note_id),
        user,
        State(state),
        Json(ReactRequest {
            content: normalize_local_reaction(&body.reaction),
        }),
    )
    .await
    .into_response();
    as_no_content(resp)
}

/// POST /api/notes/reactions/delete
/// Misskey は `noteId` のみを受け取る（1投稿1ユーザー1リアクションが前提のため対象の絵文字を
/// 指定する必要がない）。既存の `delete_reaction` は絵文字をパスパラメータに取るため、
/// ここで現在のリアクション内容を引いてから委譲する。
pub async fn reactions_delete(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<NoteIdBody>,
) -> Response {
    let user = match crate::middleware::AuthedUser::from_headers(&headers, &state).await {
        Ok(u) => u,
        Err(e) => return e,
    };
    let actor_id = user.actor_id;
    let note_id: i64 = match body.note_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let content: Option<String> =
        sqlx::query_scalar("SELECT content FROM reactions WHERE post_id = $1 AND actor_id = $2")
            .bind(note_id)
            .bind(actor_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let content = match content {
        Some(c) => c,
        None => return ApiError::NotFound("NOT_REACTED").into_response(),
    };

    let resp =
        crate::handlers::notes::delete_reaction(Path((body.note_id, content)), user, State(state))
            .await
            .into_response();
    as_no_content(resp)
}

/// POST /api/notes/unrenote
pub async fn notes_unrenote(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<NoteIdBody>,
) -> impl IntoResponse {
    let user = match crate::middleware::AuthedUser::from_headers(&headers, &state).await {
        Ok(u) => u,
        Err(e) => return as_no_content(e),
    };
    let resp = crate::handlers::notes::delete_repost(Path(body.note_id), user, State(state))
        .await
        .into_response();
    as_no_content(resp)
}

// ─── 通知 ────────────────────────────────────────────────────────────

/// POST /api/i/notifications
/// 自分宛ての通知を新しい順にカーソルページネーション取得する。以前はWebSocketの
/// プッシュ配信のみでオンメモリ保持（ページ再読み込みで消失、直近100件までしか遡れない）
/// だった「クイック通知」を永続化し、無限スクロールで過去分も遡れるようにする。
pub async fn i_notifications(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NotificationsBody>,
) -> Result<Json<Vec<MisskeyNotification>>, ApiError> {
    // includeTypes が空配列の場合は何もクエリしない（本家 Misskey の仕様）
    if body.include_types.as_ref().is_some_and(|t| t.is_empty()) {
        return Ok(Json(vec![]));
    }

    let limit = body.limit.unwrap_or(10).clamp(1, 100);
    let until_id: Option<i64> = body.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = body.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .notifications
        .list(user.actor_id, limit, until_id, since_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let rows: Vec<_> = rows
        .into_iter()
        .filter(|r| {
            if let Some(include) = &body.include_types {
                if !include.iter().any(|t| t == &r.kind) {
                    return false;
                }
            }
            if let Some(exclude) = &body.exclude_types {
                if exclude.iter().any(|t| t == &r.kind) {
                    return false;
                }
            }
            true
        })
        .collect();

    if body.mark_as_read {
        if let Err(e) = state.notifications.mark_all_read(user.actor_id).await {
            tracing::error!("[i/notifications] mark_all_read 失敗: {}", e);
        }
    }

    Ok(Json(build_notifications(&state, rows, user.actor_id).await))
}

// ─── フォロー ────────────────────────────────────────────────────────

/// POST /api/following/create
pub async fn following_create(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<FollowingBody>,
) -> Response {
    // ターゲット解決（DB問い合わせ）より先に認証を確認する。未認証のまま先に解決すると
    // 「このIDのユーザーは存在するか」を匿名で探索できてしまう（列挙攻撃対策）。
    let user = match crate::middleware::AuthedUser::from_headers(&headers, &state).await {
        Ok(u) => u,
        Err(e) => return e,
    };
    let actor_id: i64 = match body.user_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_USER_ID".to_owned()).into_response(),
    };
    let target = match actor_id_to_target(&state, actor_id).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let resp = crate::handlers::follows::create_follow(
        user,
        State(state),
        Json(CreateFollowRequest { target }),
    )
    .await
    .into_response();
    as_no_content(resp)
}

/// POST /api/following/delete
pub async fn following_delete(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<FollowingBody>,
) -> Response {
    let user = match crate::middleware::AuthedUser::from_headers(&headers, &state).await {
        Ok(u) => u,
        Err(e) => return e,
    };
    let actor_id: i64 = match body.user_id.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_USER_ID".to_owned()).into_response(),
    };
    let target = match actor_id_to_target(&state, actor_id).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let resp = crate::handlers::follows::delete_follow(
        user,
        State(state),
        Json(DeleteFollowRequest { target }),
    )
    .await
    .into_response();
    as_no_content(resp)
}

/// POST /api/users/following — 指定ユーザーのフォロー中一覧（Misskey互換、#81）。
/// カスタムAPI `GET /api/users/following`（`handlers::users::user_following`）と同じ
/// `list_following` を使い、Misskey本家の `Following` エンティティ形状に変換する。
pub async fn users_following(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<UserRelationBody>,
) -> Result<Json<Vec<MisskeyFollowRelation>>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let actor_id: i64 = body
        .user_id
        .parse()
        .map_err(|_| ApiError::NotFound("USER_NOT_FOUND"))?;
    let limit = body.limit.unwrap_or(10).clamp(1, 100);
    let until_id: Option<i64> = body.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = body.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .follows
        .list_following(actor_id, my_actor_id, limit, until_id, since_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // アクターごとに`find_by_id`+`build_user_detailed`（計4クエリ）を呼ぶと
    // limit=100件で最大400クエリになるN+1だったため、一括取得する（#81改善）。
    let actor_ids: Vec<i64> = rows.iter().map(|r| r.actor_id).collect();
    let actors = state
        .actors
        .find_by_ids(&actor_ids)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let actor_by_id: HashMap<i64, Actor> = actors.into_iter().map(|a| (a.id, a)).collect();
    let mut detailed_by_id =
        build_users_detailed(&state, &actor_by_id.values().cloned().collect::<Vec<_>>()).await;

    let mut relations = Vec::with_capacity(rows.len());
    for r in rows {
        if !actor_by_id.contains_key(&r.actor_id) {
            return Err(ApiError::NotFound("USER_NOT_FOUND"));
        }
        relations.push(MisskeyFollowRelation {
            id: r.follow_id.to_string(),
            created_at: r.created_at.to_rfc3339(),
            followee_id: r.actor_id.to_string(),
            follower_id: actor_id.to_string(),
            followee: detailed_by_id.remove(&r.actor_id),
            follower: None,
        });
    }

    Ok(Json(relations))
}

/// POST /api/users/followers — 指定ユーザーのフォロワー一覧（Misskey互換、#81）。`users_following` と対。
pub async fn users_followers(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<UserRelationBody>,
) -> Result<Json<Vec<MisskeyFollowRelation>>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let actor_id: i64 = body
        .user_id
        .parse()
        .map_err(|_| ApiError::NotFound("USER_NOT_FOUND"))?;
    let limit = body.limit.unwrap_or(10).clamp(1, 100);
    let until_id: Option<i64> = body.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = body.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .follows
        .list_followers(actor_id, my_actor_id, limit, until_id, since_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // users_following と同様、アクター件数分のN+1を避けて一括取得する（#81改善）。
    let actor_ids: Vec<i64> = rows.iter().map(|r| r.actor_id).collect();
    let actors = state
        .actors
        .find_by_ids(&actor_ids)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let actor_by_id: HashMap<i64, Actor> = actors.into_iter().map(|a| (a.id, a)).collect();
    let mut detailed_by_id =
        build_users_detailed(&state, &actor_by_id.values().cloned().collect::<Vec<_>>()).await;

    let mut relations = Vec::with_capacity(rows.len());
    for r in rows {
        if !actor_by_id.contains_key(&r.actor_id) {
            return Err(ApiError::NotFound("USER_NOT_FOUND"));
        }
        relations.push(MisskeyFollowRelation {
            id: r.follow_id.to_string(),
            created_at: r.created_at.to_rfc3339(),
            followee_id: actor_id.to_string(),
            follower_id: r.actor_id.to_string(),
            followee: None,
            follower: detailed_by_id.remove(&r.actor_id),
        });
    }

    Ok(Json(relations))
}

/// POST /api/notes/reactions — 指定リアクション種別を付けたユーザー一覧（Misskey互換、#81）。
/// Ariaで絵文字リアクションを長押しした際に呼ばれる。カスタムAPI
/// `GET /api/notes/:id/reactions/:content/actors`（`handlers::notes::reaction_actors`）と
/// 同じ `actors_for_reaction` を使う。投稿の可視性チェックも同様。
pub async fn notes_reactions(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<NotesReactionsBody>,
) -> Result<Json<Vec<MisskeyNoteReaction>>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let note_id: i64 = body
        .note_id
        .parse()
        .map_err(|_| ApiError::NotFound("NOTE_NOT_FOUND"))?;

    state
        .posts
        .find_by_id_for_viewer(note_id, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOTE_NOT_FOUND"))?;

    let Some(reaction_type) = body.reaction_type else {
        return Ok(Json(vec![]));
    };
    let limit = body.limit.unwrap_or(10).clamp(1, 100);

    let actors = state
        .reactions
        .actors_for_reaction(note_id, &reaction_type, limit)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(
        actors
            .into_iter()
            .map(|a| MisskeyNoteReaction {
                id: a.reaction_id.to_string(),
                created_at: a.reaction_created_at.to_rfc3339(),
                user: user_lite(
                    a.id,
                    &a.username,
                    &a.domain,
                    a.actor_type == "local",
                    &state.local_domain,
                    a.display_name.as_deref(),
                    a.avatar_url.as_deref(),
                ),
                kind: reaction_type.clone(),
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::normalize_local_reaction;

    #[test]
    fn normalizes_misskey_local_custom_emoji_reaction() {
        assert_eq!(normalize_local_reaction(":blob_cat@.:"), ":blob_cat:");
    }

    #[test]
    fn leaves_unicode_and_canonical_reactions_unchanged() {
        assert_eq!(normalize_local_reaction("🎉"), "🎉");
        assert_eq!(normalize_local_reaction(":blob_cat:"), ":blob_cat:");
        assert_eq!(
            normalize_local_reaction(":blob_cat@example.com:"),
            ":blob_cat@example.com:"
        );
    }
}
