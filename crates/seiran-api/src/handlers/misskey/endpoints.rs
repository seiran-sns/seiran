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
        "announcements",
        "ap/show",
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
        "notes/mentions",
        "notes/polls/vote",
        "notes/reactions",
        "notes/reactions/create",
        "notes/reactions/delete",
        "notes/search",
        "notes/search-by-tag",
        "notes/show",
        "notes/timeline",
        "notes/unrenote",
        "notes/user-list-timeline",
        "stats",
        "users/clips",
        "users/featured-notes",
        "users/flashs",
        "users/followers",
        "users/following",
        "users/gallery/posts",
        "users/lists/list",
        "users/lists/show",
        "users/notes",
        "users/pages",
        "users/reactions",
        "users/show",
    ])
}
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
    MisskeyNotification, MisskeyStats, MisskeyUserDetailed, MisskeyUserList, MisskeyUserLite,
    MisskeyUserReaction,
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
pub struct NotesMentionsBody {
    pub visibility: Option<String>,
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

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UserShowBody {
    pub user_id: Option<String>,
    /// 複数ID一括取得（`MisskeyUsers.showByIds`、リストメンバー一覧画面等）。指定時は
    /// `user_id`/`username`より優先し、レスポンスも単一オブジェクトではなく配列になる
    /// （本家Misskey準拠）。
    pub user_ids: Option<Vec<String>>,
    pub username: Option<String>,
    pub host: Option<String>,
}

/// `POST /api/users/show`のレスポンス。`userIds`未指定時は単一オブジェクト、指定時は
/// 配列（本家Misskey準拠）。`misskey_dart`側の`MisskeyUsers.show`/`showByIds`はそれぞれ
/// 対応する形しかデコードしないため、`#[serde(untagged)]`で呼び出し方に応じた形を返す。
#[derive(Serialize)]
#[serde(untagged)]
pub enum UsersShowResponse {
    Single(Box<MisskeyUserDetailed>),
    Many(Vec<MisskeyUserDetailed>),
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
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<UserShowBody>,
) -> Result<Json<UsersShowResponse>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;

    if let Some(uids) = body.user_ids {
        let ids: Vec<i64> = uids.iter().filter_map(|s| s.parse::<i64>().ok()).collect();
        let actors = state
            .actors
            .find_by_ids(&ids)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        for actor in actors.iter().filter(|a| a.actor_type != "local") {
            state.enqueue_remote_profile_refresh(actor.id).await;
        }
        let mut detailed_by_id = build_users_detailed(&state, &actors, my_actor_id).await;
        // 呼び出し側が渡した順序を保つ（見つからなかったIDは結果から除外、本家Misskey準拠）。
        let ordered: Vec<MisskeyUserDetailed> = ids
            .into_iter()
            .filter_map(|id| detailed_by_id.remove(&id))
            .collect();
        return Ok(Json(UsersShowResponse::Many(ordered)));
    }

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

    // リモートアクターのプロフィール（avatar_url/banner_url等）再取得。カスタムAPI
    // （handlers::users::user_profile）と同じ「表示時再検証」パターン。
    if actor.actor_type != "local" {
        state.enqueue_remote_profile_refresh(actor.id).await;
    }

    Ok(Json(UsersShowResponse::Single(Box::new(
        build_user_detailed(&state, &actor, my_actor_id).await,
    ))))
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApShowBody {
    pub uri: String,
}

/// `type`（`"Note"`/`"User"`）＋`object`（フルオブジェクト）を本家Misskey準拠の
/// `{"type": "...", "object": {...}}`形へ組み立てる。
#[derive(Serialize)]
#[serde(tag = "type", content = "object")]
pub enum ApShowResponse {
    Note(Box<MisskeyNote>),
    User(Box<MisskeyUserDetailed>),
}

/// POST /api/ap/show（`MisskeyAp.show`）— Ariaの「ほかのアカウントで開く」機能で、
/// 他のMisskeyサーバー等で見ているノート/ユーザーを自分（seiran）のアカウントで開き直す
/// 際に呼ばれる。`uri`（AP ID・URL・`@user@host`・AT URI等）を解決し、既存の「開く」機能
/// （`handlers::open_target::resolve_open_target`、カスタムAPI`POST /api/open-target`と
/// 共通、ローカルDBに無ければフェッチ・取り込みまで行う）でNote/Userのどちらかを特定し、
/// 本家Misskey準拠の`{type, object}`で返す（#251続き）。
pub async fn ap_show(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ApShowBody>,
) -> Result<Json<ApShowResponse>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let resolved = crate::handlers::open_target::resolve_open_target(&state, &body.uri).await?;

    Ok(Json(match resolved {
        crate::handlers::open_target::ResolvedTarget::Actor(actor) => {
            if actor.actor_type != "local" {
                state.enqueue_remote_profile_refresh(actor.id).await;
            }
            let detailed = build_user_detailed(&state, &actor, my_actor_id).await;
            ApShowResponse::User(Box::new(detailed))
        }
        crate::handlers::open_target::ResolvedTarget::Post(post_id) => {
            let post = state
                .posts
                .find_by_id_for_viewer(post_id, my_actor_id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .ok_or(ApiError::NotFound("NO_SUCH_OBJECT"))?;
            ApShowResponse::Note(Box::new(build_note(&state, post, my_actor_id).await))
        }
    }))
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

/// POST /api/notes/mentions（Aria等の通知画面「メンション」「指名」タブ用。要ログイン）。
/// `visibility`未指定＝メンション全般（本文中の`@username`メンション・自分への返信・自分宛
/// directのいずれか）、`visibility: "specified"`（Misskey本家の`direct`表記）指定時は自分宛
/// direct投稿のみに絞る。詳細: `docs/protocols.md` 7節。
pub async fn notes_mentions(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<NotesMentionsBody>,
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

    let specified_only = body.visibility.as_deref() == Some("specified");
    let limit = body.limit.unwrap_or(10).clamp(1, 100);
    let until_id: Option<i64> = body.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = body.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .posts
        .mentions_timeline(actor_id, specified_only, limit, until_id, since_id)
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesPollsVoteBody {
    pub note_id: String,
    pub choice: usize,
}

/// POST /api/notes/polls/vote — アンケート投票（Aria等、`MisskeyNotesPolls.vote`）。
/// 既存のカスタムAPI `POST /api/notes/:id/poll-vote`（`handlers::notes::poll::vote_poll`）を
/// そのまま呼び出し、成功時のレスポンスだけMisskey流（204 No Content）に整形する
/// （#252続き）。本家Misskeyは複数選択のアンケートでも1回の呼び出しにつき選択肢1つ
/// （`choice`、単数形）のみを送るため、そのまま`option_indexes: vec![choice]`へ変換する。
pub async fn notes_polls_vote(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<NotesPollsVoteBody>,
) -> impl IntoResponse {
    let user = match crate::middleware::AuthedUser::from_headers(&headers, &state).await {
        Ok(u) => u,
        Err(e) => return as_no_content(e),
    };
    let resp = crate::handlers::notes::poll::vote_poll(
        Path(body.note_id),
        user,
        State(state),
        Json(crate::handlers::notes::poll::PollVoteRequest {
            option_indexes: vec![body.choice],
        }),
    )
    .await
    .into_response();
    as_no_content(resp)
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
            content: body.reaction,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserListTimelineBody {
    pub list_id: String,
    pub limit: Option<i64>,
    pub since_id: Option<String>,
    pub until_id: Option<String>,
}

/// POST /api/notes/user-list-timeline — リストタイムライン画面（Aria等）。リスト一覧
/// （`users/lists/list`）は実装済みだったが、個別のリストを開くこの取得系エンドポイントが
/// 無く404になっていた。カスタムAPI `GET /api/lists/:id/timeline`
/// （`handlers::lists::list_timeline`）と同じ`ListRepository::timeline`・公開範囲チェック
/// （非公開リストは所有者本人のみ）を使う。WebSocketの`userList`チャンネル購読は既存実装
/// （`docs/protocols.md`参照）でカバー済みで、こちらは画面を開いた際の初回一覧取得を担う。
pub async fn notes_user_list_timeline(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<UserListTimelineBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let list_id: i64 = body
        .list_id
        .parse()
        .map_err(|_| ApiError::NotFound("NO_SUCH_LIST"))?;

    let row = state
        .lists
        .find_by_id(list_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NO_SUCH_LIST"))?;
    if !row.is_public && my_actor_id != Some(row.owner_actor_id) {
        return Err(ApiError::NotFound("NO_SUCH_LIST"));
    }

    let limit = body.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = body.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = body.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .lists
        .timeline(list_id, limit, until_id, since_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
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
        State(state.clone()),
        Json(CreateFollowRequest { target }),
    )
    .await
    .into_response();
    if !resp.status().is_success() {
        return resp;
    }
    misskey_user_lite_response(&state, actor_id).await
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
        State(state.clone()),
        Json(DeleteFollowRequest { target }),
    )
    .await
    .into_response();
    if !resp.status().is_success() {
        return resp;
    }
    misskey_user_lite_response(&state, actor_id).await
}

/// `following/create`・`following/delete`成功時に返す`UserLite`。本家Misskeyはこれらを
/// 204ではなく対象ユーザーの`UserLite`で応答する仕様で、misskey_dartの
/// `MisskeyFollowing.create`/`delete`は`post<Map<String, dynamic>>`で直接キャストする
/// （204 No Contentのまま返すと、空ボディがJSONデコードで文字列扱いになり
/// `type 'String' is not a subtype of type 'FutureOr<Map<String, dynamic>>'`で
/// クライアント側が例外落ちする。実機確認済み）。
async fn misskey_user_lite_response(state: &AppState, actor_id: i64) -> Response {
    match state.actors.find_by_id(actor_id).await {
        Ok(Some(actor)) => {
            let lite: MisskeyUserLite = build_user_detailed(state, &actor, None).await.lite;
            Json(lite).into_response()
        }
        _ => StatusCode::NO_CONTENT.into_response(),
    }
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
    let mut detailed_by_id = build_users_detailed(
        &state,
        &actor_by_id.values().cloned().collect::<Vec<_>>(),
        my_actor_id,
    )
    .await;

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
    let mut detailed_by_id = build_users_detailed(
        &state,
        &actor_by_id.values().cloned().collect::<Vec<_>>(),
        my_actor_id,
    )
    .await;

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
        .actors_for_reaction(note_id, &reaction_type, my_actor_id, limit)
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

/// POST /api/users/reactions — プロフィール「リアクション」タブ（Aria等）。
/// カスタムAPI側のプロフィール混合フィード（`handlers::users::user_posts`）と同じ
/// `reactions_by_actor_for_feed` を使い、対象ノートは`fetch_referenced_notes`と同じ
/// 一括取得・可視性フィルタで埋め込む。対象ノートが削除済み・非公開等で取得できない行は
/// （本家Misskeyも閲覧不可なノートへのリアクションは返さないため）結果から除外する。
pub async fn users_reactions(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<UsersNotesBody>,
) -> Result<Json<Vec<MisskeyUserReaction>>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let actor_id: i64 = body
        .user_id
        .parse()
        .map_err(|_| ApiError::NotFound("USER_NOT_FOUND"))?;
    let actor = state
        .actors
        .find_by_id(actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("USER_NOT_FOUND"))?;
    let limit = body.limit.unwrap_or(10).clamp(1, 100);
    let until_id: Option<i64> = body.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = body.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .reactions
        .reactions_by_actor_for_feed(actor_id, my_actor_id, until_id, since_id, true, limit)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut post_ids: Vec<i64> = rows.iter().map(|r| r.post_id).collect();
    post_ids.sort_unstable();
    post_ids.dedup();
    let notes_by_id = super::convert::fetch_referenced_notes(&state, &post_ids, my_actor_id).await;
    let user = build_user_detailed(&state, &actor, my_actor_id).await.lite;

    Ok(Json(
        rows.into_iter()
            .filter_map(|r| {
                let note = notes_by_id.get(&r.post_id)?.clone();
                Some(MisskeyUserReaction {
                    id: r.id.to_string(),
                    created_at: r.created_at.to_rfc3339(),
                    user: user.clone(),
                    kind: r.content,
                    note,
                })
            })
            .collect(),
    ))
}

/// POST /api/stats。ローカルの投稿数・ユーザー数のみ実数を返す（#251）。退会済みユーザー
/// （`actors.withdrawn_at`）・削除済みポスト（`posts.deleted_at`）・リモートポストは集計
/// 対象外。`instances`（既知フェディバースサーバー数）・`driveUsageLocal`/`driveUsageRemote`
/// は未実装のため引き続き0を返す（キー自体を省略すると`misskey_dart`が必須フィールド
/// 欠落として例外を投げるため、値が0でもキーは揃える）。リモートを一切集計しないため
/// `notesCount`/`usersCount`と`originalNotesCount`/`originalUsersCount`は常に同値になる。
pub async fn stats(State(state): State<AppState>) -> Result<Json<MisskeyStats>, ApiError> {
    let notes_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM posts p
         JOIN actors a ON a.id = p.actor_id
         WHERE a.actor_type = 'local' AND p.deleted_at IS NULL",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let users_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM actors WHERE actor_type = 'local' AND withdrawn_at IS NULL",
    )
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(MisskeyStats {
        notes_count,
        original_notes_count: notes_count,
        users_count,
        original_users_count: users_count,
        ..MisskeyStats::default()
    }))
}

/// 未実装のMisskey機能（お知らせ・ハイライト`users/featured-notes`・クリップ・
/// ページ・Play・ギャラリー）用の共通スタブ。本文は検証せず無視し、常に空配列を返す
/// （#251、Aria非互換修正）。これらの機能自体が存在しない/エンドポイントが無いと
/// `misskey_dart`が404として例外を投げ、プロフィール等の該当タブがエラー表示になる
/// （実機確認、Aria）。「リスト」は`users_lists_list`が実データを返すため対象外。
pub async fn empty_list_stub(
    body: Option<Json<serde_json::Value>>,
) -> Json<Vec<serde_json::Value>> {
    let _ = body;
    Json(Vec::new())
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsersListsListBody {
    pub user_id: Option<String>,
}

/// `ListRow`一件を、メンバー一覧付き`MisskeyUserList`へ組み立てる（`users_lists_list`・
/// `users_lists_show`共通）。
async fn build_misskey_user_list(
    state: &AppState,
    row: seiran_common::repository::ListRow,
) -> Result<MisskeyUserList, ApiError> {
    let members = state
        .lists
        .members(row.id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(MisskeyUserList {
        id: row.id.to_string(),
        created_at: row.created_at.to_rfc3339(),
        name: row.name,
        is_public: row.is_public,
        user_ids: members
            .into_iter()
            .map(|m| m.actor_id.to_string())
            .collect(),
    })
}

/// POST /api/users/lists/list — プロフィール「リスト」タブ・自分のリスト管理画面（Aria等）。
/// `userId`指定時はそのユーザーの公開リストのみ（本家Misskey準拠、プロフィール表示から
/// 他人のリストを覗く用途）、省略時は認証ユーザー自身の全リスト（非公開含む、自分のリスト
/// 管理画面用途）を返す。既存のカスタムAPI（`handlers::lists`）と同じ`ListRepository`を使う
/// （#251続き）。
pub async fn users_lists_list(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Option<Json<UsersListsListBody>>,
) -> Result<Json<Vec<MisskeyUserList>>, ApiError> {
    let user_id = body.and_then(|Json(b)| b.user_id);

    let rows = if let Some(uid) = user_id {
        let actor_id: i64 = uid
            .parse()
            .map_err(|_| ApiError::NotFound("USER_NOT_FOUND"))?;
        state.lists.list_public_by_owner(actor_id).await
    } else {
        let my_actor_id = optional_actor_id(&headers, &state)
            .await
            .ok_or(ApiError::Unauthorized("CREDENTIAL_REQUIRED"))?;
        state.lists.list_by_owner(my_actor_id).await
    }
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(build_misskey_user_list(&state, row).await?);
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsersListsShowBody {
    pub list_id: String,
}

/// POST /api/users/lists/show — リストを開いた詳細画面（Aria等、`MisskeyUsersLists.show`）。
/// `users/lists/list`（一覧）は実装済みでも、個別のリストの名前・メンバー等を取得する
/// このエンドポイントが無く404になっていた。カスタムAPI `GET /api/lists/:id`
/// （`handlers::lists`）と同じ公開範囲チェック（非公開リストは所有者本人のみ、
/// `NO_SUCH_LIST`）を使う（#251続き）。
pub async fn users_lists_show(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<UsersListsShowBody>,
) -> Result<Json<MisskeyUserList>, ApiError> {
    let my_actor_id = optional_actor_id(&headers, &state).await;
    let list_id: i64 = body
        .list_id
        .parse()
        .map_err(|_| ApiError::NotFound("NO_SUCH_LIST"))?;

    let row = state
        .lists
        .find_by_id(list_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NO_SUCH_LIST"))?;
    if !row.is_public && my_actor_id != Some(row.owner_actor_id) {
        return Err(ApiError::NotFound("NO_SUCH_LIST"));
    }

    Ok(Json(build_misskey_user_list(&state, row).await?))
}
