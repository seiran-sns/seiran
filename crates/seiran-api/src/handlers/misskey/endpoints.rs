//! Misskey 実物のパス・POSTオンリー規約に合わせた**追加**エンドポイント。
//!
//! 書き込み系（リアクション作成/削除・リノート取消・フォロー作成/削除）は既存の
//! `handlers::notes`/`handlers::follows` の関数を直接呼び出して副作用（AP/ATP配送・
//! ストリーミング配信）ロジックを再利用し、成功時のレスポンスだけ Misskey 流
//! （`204 No Content`）に整形する。エラー時は既存の `ApiError` 形状をそのまま返す
//! （Misskey 本家のエラーID/種別は再現していない。将来の課題）。

use axum::{
    extract::{Path, State},
    http::StatusCode,
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

use seiran_common::repository::{Actor, FollowListRow, Page};

use crate::error::ApiError;
use crate::handlers::follows::FollowTargetRequest;
use crate::handlers::notes::ReactRequest;
use crate::middleware::{AuthedUser, MaybeAuthedUser};
use crate::AppState;

use super::convert::{
    build_me_detailed, build_note, build_notes, build_notifications, build_user_detailed,
    build_users_detailed, user_lite, ActorSummary,
};
use super::types::{
    MisskeyFollowRelation, MisskeyMeDetailed, MisskeyNote, MisskeyNoteReaction,
    MisskeyNotification, MisskeyStats, MisskeyUserDetailed, MisskeyUserList, MisskeyUserLite,
    MisskeyUserReaction,
};

// ─── リクエストDTO（Misskey 本家の camelCase フィールド名に合わせる） ──────────

/// Misskey 共通のカーソル指定（`limit`/`sinceId`/`untilId`）。各リクエストボディに
/// `#[serde(flatten)]`で埋め込み、`page`で`Page`へ正規化する。
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CursorParams {
    pub limit: Option<i64>,
    pub since_id: Option<String>,
    pub until_id: Option<String>,
}

impl CursorParams {
    /// `limit`は1〜100に丸め、未指定なら`default_limit`。数値として解釈できないIDは無視する。
    fn page(&self, default_limit: i64) -> Page {
        let parse = |id: &Option<String>| id.as_deref().and_then(|s| s.parse::<i64>().ok());
        Page {
            limit: self.limit.unwrap_or(default_limit).clamp(1, 100),
            until_id: parse(&self.until_id),
            since_id: parse(&self.since_id),
        }
    }
}

#[derive(Deserialize, Default)]
pub struct TimelineBody {
    #[serde(flatten)]
    pub cursor: CursorParams,
}

#[derive(Deserialize)]
pub struct NotesMentionsBody {
    pub visibility: Option<String>,
    #[serde(flatten)]
    pub cursor: CursorParams,
}

#[derive(Deserialize)]
pub struct NotesSearchBody {
    pub query: String,
    #[serde(flatten)]
    pub cursor: CursorParams,
}

#[derive(Deserialize)]
pub struct NotesSearchByTagBody {
    pub tag: String,
    #[serde(flatten)]
    pub cursor: CursorParams,
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

/// `POST /api/users/notes`・`users/reactions`・`users/following`・`users/followers` 共通の
/// リクエストボディ（#81）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserCursorBody {
    pub user_id: String,
    #[serde(flatten)]
    pub cursor: CursorParams,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowingBody {
    pub user_id: String,
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
    #[serde(flatten)]
    pub cursor: CursorParams,
    #[serde(default = "default_true")]
    pub mark_as_read: bool,
    pub include_types: Option<Vec<String>>,
    pub exclude_types: Option<Vec<String>>,
}

// ─── 共通ヘルパー ───────────────────────────────────────────────────────

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

/// Misskey の `userId`/`noteId`/`listId`（文字列）を数値IDへ変換する。解釈できなければ
/// `not_found`（本家Misskeyは存在しないIDと同じエラーを返す）。
fn parse_id(id: &str, not_found: &'static str) -> Result<i64, ApiError> {
    id.parse().map_err(|_| ApiError::NotFound(not_found))
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

/// リモートアクターのプロフィール（avatar_url/banner_url等）再取得。カスタムAPI
/// （handlers::users::user_profile）と同じ「表示時再検証」パターン。
async fn refresh_if_remote(state: &AppState, actor: &Actor) {
    if actor.actor_type != "local" {
        state.enqueue_remote_profile_refresh(actor.id).await;
    }
}

// ─── 自分自身・ユーザー ─────────────────────────────────────────────────

/// POST /api/i
pub async fn api_i(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<MisskeyMeDetailed>, ApiError> {
    let actor = state
        .actors
        .find_by_id(user.actor_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;
    Ok(Json(build_me_detailed(&state, &actor).await))
}

/// POST /api/users/show
pub async fn users_show(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<UserShowBody>,
) -> Result<Json<UsersShowResponse>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);

    if let Some(uids) = body.user_ids {
        let ids: Vec<i64> = uids.iter().filter_map(|s| s.parse::<i64>().ok()).collect();
        let actors = state.actors.find_by_ids(&ids).await.map_err(internal)?;
        for actor in &actors {
            refresh_if_remote(&state, actor).await;
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
        let id = parse_id(&uid, "USER_NOT_FOUND")?;
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
    .map_err(internal)?
    .ok_or(ApiError::NotFound("USER_NOT_FOUND"))?;

    refresh_if_remote(&state, &actor).await;
    Ok(Json(UsersShowResponse::Single(Box::new(
        build_user_detailed(&state, &actor, my_actor_id).await,
    ))))
}

/// POST /api/users/notes — プロフィール画面のノートタブ（Aria等）。
/// カスタムAPI `GET /api/users/posts`（`handlers::users::user_posts`）と同じ
/// `timeline_by_actor` を使うが、`exclude_direct=true` はカスタムAPI側の
/// `build_profile_response` 初回取得と同じ扱い（DMをプロフィール投稿一覧に含めない）。
pub async fn users_notes(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<UserCursorBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);
    let actor_id = parse_id(&body.user_id, "USER_NOT_FOUND")?;
    let page = body.cursor.page(10);
    let rows = state
        .posts
        .timeline_by_actor(
            actor_id,
            my_actor_id,
            page.limit,
            page.until_id,
            page.since_id,
            true,
        )
        .await
        .map_err(internal)?;
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
}

// ─── ノート ──────────────────────────────────────────────────────────

/// POST /api/notes/show
pub async fn notes_show(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NoteIdBody>,
) -> Result<Json<MisskeyNote>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);
    let post_id = parse_id(&body.note_id, "NOTE_NOT_FOUND")?;
    let post = state
        .posts
        .find_by_id_for_viewer(post_id, my_actor_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("NOTE_NOT_FOUND"))?;
    Ok(Json(build_note(&state, post, my_actor_id).await))
}

#[derive(Deserialize)]
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
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<ApShowBody>,
) -> Result<Json<ApShowResponse>, ApiError> {
    use crate::handlers::open_target::{resolve_open_target, ResolvedTarget};

    let my_actor_id = me.map(|u| u.actor_id);
    Ok(Json(match resolve_open_target(&state, &body.uri).await? {
        ResolvedTarget::Actor(actor) => {
            refresh_if_remote(&state, &actor).await;
            let detailed = build_user_detailed(&state, &actor, my_actor_id).await;
            ApShowResponse::User(Box::new(detailed))
        }
        ResolvedTarget::Post(post_id) => {
            let post = state
                .posts
                .find_by_id_for_viewer(post_id, my_actor_id)
                .await
                .map_err(internal)?
                .ok_or(ApiError::NotFound("NO_SUCH_OBJECT"))?;
            ApShowResponse::Note(Box::new(build_note(&state, post, my_actor_id).await))
        }
    }))
}

// Misskey互換APIのタイムラインはMisskey本家の`specified`同様のデフォルト挙動を保つため、
// `exclude_direct`は常に`false`（自分宛のdirectは含まれる）。

/// POST /api/notes/local-timeline
pub async fn notes_local_timeline(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<TimelineBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);
    let page = body.cursor.page(20);
    let rows = state
        .posts
        .local_timeline(my_actor_id, page.limit, page.until_id, page.since_id, false)
        .await
        .map_err(internal)?;
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
}

/// POST /api/notes/timeline（ホームタイムライン。要ログイン）
pub async fn notes_home_timeline(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<TimelineBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let page = body.cursor.page(30);
    let rows = state
        .posts
        .home_timeline(
            user.actor_id,
            page.limit,
            page.until_id,
            page.since_id,
            false,
        )
        .await
        .map_err(internal)?;
    Ok(Json(build_notes(&state, rows, Some(user.actor_id)).await))
}

/// POST /api/notes/mentions（Aria等の通知画面「メンション」「指名」タブ用。要ログイン）。
/// `visibility`未指定＝メンション全般（本文中の`@username`メンション・自分への返信・自分宛
/// directのいずれか）、`visibility: "specified"`（Misskey本家の`direct`表記）指定時は自分宛
/// direct投稿のみに絞る。詳細: `docs/protocols.md` 7節。
pub async fn notes_mentions(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NotesMentionsBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let specified_only = body.visibility.as_deref() == Some("specified");
    let page = body.cursor.page(10);
    let rows = state
        .posts
        .mentions_timeline(
            user.actor_id,
            specified_only,
            page.limit,
            page.until_id,
            page.since_id,
        )
        .await
        .map_err(internal)?;
    Ok(Json(build_notes(&state, rows, Some(user.actor_id)).await))
}

/// POST /api/notes/hybrid-timeline（ソーシャルタイムライン。要ログイン、#78）
pub async fn notes_hybrid_timeline(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<TimelineBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let page = body.cursor.page(30);
    let rows = state
        .posts
        .social_timeline(
            user.actor_id,
            page.limit,
            page.until_id,
            page.since_id,
            false,
        )
        .await
        .map_err(internal)?;
    Ok(Json(build_notes(&state, rows, Some(user.actor_id)).await))
}

/// POST /api/notes/global-timeline（グローバルタイムライン、#78）
pub async fn notes_global_timeline(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<TimelineBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);
    let page = body.cursor.page(20);
    let rows = state
        .posts
        .global_timeline(my_actor_id, page.limit, page.until_id, page.since_id, false)
        .await
        .map_err(internal)?;
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
}

/// POST /api/notes/search-by-tag（Misskey互換、Ariaのハッシュタグ画面）。
pub async fn notes_search_by_tag(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NotesSearchByTagBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let tag = body.tag.trim().trim_start_matches('#').to_lowercase();
    if tag.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let my_actor_id = me.map(|u| u.actor_id);
    let page = body.cursor.page(30);
    let rows = state
        .hashtags
        .timeline(&tag, page.limit, page.until_id, page.since_id, my_actor_id)
        .await
        .map_err(internal)?;
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
}

/// POST /api/notes/search（Misskey互換、Aria等）。検索対象の決定はカスタムAPI
/// （`GET /api/notes/search`）と同じ`search_post_ids_by_cursor`・検索回数制限を使う。
pub async fn notes_search(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NotesSearchBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let query = body.query.trim();
    if query.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let page = body.cursor.page(30);
    if let Some(user) = &me {
        let is_initial_search = page.until_id.is_none() && page.since_id.is_none();
        crate::rate_limit::check_search_rate_limit(&state, user.actor_id, is_initial_search)
            .await?;
    }
    let my_actor_id = me.as_ref().map(|u| u.actor_id);
    let viewer = me.as_ref().map(|u| (u.actor_id, u.username.as_str()));
    let ids = crate::handlers::search::search_post_ids_by_cursor(
        &state,
        query,
        page.limit as usize,
        page.until_id,
        page.since_id,
        viewer,
    )
    .await;
    let mut rows =
        seiran_common::repository::find_visible_posts_by_ids(&state.db, &ids, my_actor_id)
            .await
            .map_err(internal)?;
    rows.sort_unstable_by_key(|p| std::cmp::Reverse(p.id));
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
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NotesPollsVoteBody>,
) -> Response {
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
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<ReactionCreateBody>,
) -> Response {
    let resp = crate::handlers::notes::create_reaction(
        Path(body.note_id),
        user,
        State(state),
        Json(ReactRequest {
            content: body.reaction,
        }),
    )
    .await;
    as_no_content(resp)
}

/// POST /api/notes/reactions/delete
/// Misskey は `noteId` のみを受け取る（1投稿1ユーザー1リアクションが前提のため対象の絵文字を
/// 指定する必要がない）。カスタムAPIと同じ取り消し処理（`remove_reaction`）を、内容を問わない
/// 指定（`None`）で呼ぶ（内容を先に読んでから渡すと、同時の切り替えと競合して取り消せない）。
pub async fn reactions_delete(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NoteIdBody>,
) -> Response {
    let resp =
        crate::handlers::notes::reactions::remove_reaction(&state, &user, &body.note_id, None)
            .await;
    as_no_content(resp)
}

/// POST /api/notes/unrenote
pub async fn notes_unrenote(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NoteIdBody>,
) -> Response {
    let resp = crate::handlers::notes::delete_repost(Path(body.note_id), user, State(state))
        .await
        .into_response();
    as_no_content(resp)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserListTimelineBody {
    pub list_id: String,
    #[serde(flatten)]
    pub cursor: CursorParams,
}

/// 閲覧可能なリストを取得する（非公開リストは所有者本人のみ、カスタムAPI
/// `handlers::lists`と同じ公開範囲チェック、`NO_SUCH_LIST`）。
async fn find_viewable_list(
    state: &AppState,
    list_id: &str,
    my_actor_id: Option<i64>,
) -> Result<seiran_common::repository::ListRow, ApiError> {
    let list_id = parse_id(list_id, "NO_SUCH_LIST")?;
    let row = state
        .lists
        .find_by_id(list_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("NO_SUCH_LIST"))?;
    if !row.is_public && my_actor_id != Some(row.owner_actor_id) {
        return Err(ApiError::NotFound("NO_SUCH_LIST"));
    }
    Ok(row)
}

/// POST /api/notes/user-list-timeline — リストタイムライン画面（Aria等）。カスタムAPI `GET /api/lists/:id/timeline`
/// （`handlers::lists::list_timeline`）と同じ`ListRepository::timeline`・公開範囲チェック
/// （非公開リストは所有者本人のみ）を使う。WebSocketの`userList`チャンネル購読は既存実装
/// （`docs/protocols.md`参照）でカバー済みで、こちらは画面を開いた際の初回一覧取得を担う。
pub async fn notes_user_list_timeline(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<UserListTimelineBody>,
) -> Result<Json<Vec<MisskeyNote>>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);
    let list = find_viewable_list(&state, &body.list_id, my_actor_id).await?;
    let page = body.cursor.page(20);
    let rows = state
        .lists
        .timeline(list.id, page.limit, page.until_id, page.since_id)
        .await
        .map_err(internal)?;
    Ok(Json(build_notes(&state, rows, my_actor_id).await))
}

// ─── 通知 ────────────────────────────────────────────────────────────

/// POST /api/i/notifications
/// 自分宛ての通知を新しい順にカーソルページネーション取得する。
pub async fn i_notifications(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NotificationsBody>,
) -> Result<Json<Vec<MisskeyNotification>>, ApiError> {
    // includeTypes が空配列の場合は何もクエリしない（本家 Misskey の仕様）
    if body.include_types.as_ref().is_some_and(|t| t.is_empty()) {
        return Ok(Json(vec![]));
    }

    let page = body.cursor.page(10);
    let rows = state
        .notifications
        .list(user.actor_id, page.limit, page.until_id, page.since_id)
        .await
        .map_err(internal)?;

    let rows: Vec<_> = rows
        .into_iter()
        .filter(|r| {
            body.include_types
                .as_ref()
                .is_none_or(|include| include.iter().any(|t| t == &r.kind))
                && !body
                    .exclude_types
                    .as_ref()
                    .is_some_and(|exclude| exclude.iter().any(|t| t == &r.kind))
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
/// 認証（`AuthedUser`抽出子）はターゲット解決（DB問い合わせ）より先に行われる。未認証のまま
/// 先に解決すると「このIDのユーザーは存在するか」を匿名で探索できてしまう（列挙攻撃対策）。
pub async fn following_create(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<FollowingBody>,
) -> Result<Response, ApiError> {
    let actor_id: i64 = body
        .user_id
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_USER_ID".to_owned()))?;
    let resp = crate::handlers::follows::create_follow(
        user,
        State(state.clone()),
        Json(FollowTargetRequest {
            actor_id: Some(body.user_id),
            ..FollowTargetRequest::default()
        }),
    )
    .await
    .into_response();
    if !resp.status().is_success() {
        return Ok(resp);
    }
    Ok(misskey_user_lite_response(&state, actor_id).await)
}

/// POST /api/following/delete
pub async fn following_delete(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(body): Json<FollowingBody>,
) -> Result<Response, ApiError> {
    let actor_id: i64 = body
        .user_id
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_USER_ID".to_owned()))?;
    let resp = crate::handlers::follows::delete_follow(
        user,
        State(state.clone()),
        Json(FollowTargetRequest {
            actor_id: Some(body.user_id),
            ..FollowTargetRequest::default()
        }),
    )
    .await
    .into_response();
    if !resp.status().is_success() {
        return Ok(resp);
    }
    Ok(misskey_user_lite_response(&state, actor_id).await)
}

/// `following/create`・`following/delete`成功時に返す`UserLite`。本家Misskeyはこれらを
/// 204ではなく対象ユーザーの`UserLite`で応答する仕様で、misskey_dartの
/// `MisskeyFollowing.create`/`delete`は`post<Map<String, dynamic>>`で直接キャストする
/// （204 No Contentのまま返すと、空ボディがJSONデコードで文字列扱いになり
/// `type 'String' is not a subtype of type 'FutureOr<Map<String, dynamic>>'`で
/// クライアント側が例外落ちする）。
async fn misskey_user_lite_response(state: &AppState, actor_id: i64) -> Response {
    match state.actors.find_by_id(actor_id).await {
        Ok(Some(actor)) => {
            let lite: MisskeyUserLite = build_user_detailed(state, &actor, None).await.lite;
            Json(lite).into_response()
        }
        _ => StatusCode::NO_CONTENT.into_response(),
    }
}

/// フォロー一覧の向き（`users/following`・`users/followers`）。
#[derive(Clone, Copy)]
enum FollowDirection {
    /// 指定ユーザーがフォローしている相手の一覧。
    Following,
    /// 指定ユーザーをフォローしている相手の一覧。
    Followers,
}

/// `users/following`・`users/followers` 共通。カスタムAPI（`handlers::users::user_following`/
/// `user_followers`）と同じ `list_following`/`list_followers` を使い、Misskey本家の
/// `Following` エンティティ形状に変換する。アクターごとに`find_by_id`+`build_user_detailed`
/// （計4クエリ）を呼ぶと limit=100件で最大400クエリになるN+1だったため、一括取得する（#81改善）。
async fn follow_relations(
    state: &AppState,
    me: Option<AuthedUser>,
    body: UserCursorBody,
    direction: FollowDirection,
) -> Result<Json<Vec<MisskeyFollowRelation>>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);
    let actor_id = parse_id(&body.user_id, "USER_NOT_FOUND")?;
    let page = body.cursor.page(10);
    let rows: Vec<FollowListRow> = match direction {
        FollowDirection::Following => {
            state
                .follows
                .list_following(
                    actor_id,
                    my_actor_id,
                    page.limit,
                    page.until_id,
                    page.since_id,
                )
                .await
        }
        FollowDirection::Followers => {
            state
                .follows
                .list_followers(
                    actor_id,
                    my_actor_id,
                    page.limit,
                    page.until_id,
                    page.since_id,
                )
                .await
        }
    }
    .map_err(internal)?;

    let actor_ids: Vec<i64> = rows.iter().map(|r| r.actor_id).collect();
    let actors = state
        .actors
        .find_by_ids(&actor_ids)
        .await
        .map_err(internal)?;
    let mut detailed_by_id = build_users_detailed(state, &actors, my_actor_id).await;

    rows.into_iter()
        .map(|r| {
            let other = detailed_by_id
                .remove(&r.actor_id)
                .ok_or(ApiError::NotFound("USER_NOT_FOUND"))?;
            let (followee_id, follower_id, followee, follower) = match direction {
                FollowDirection::Following => (r.actor_id, actor_id, Some(other), None),
                FollowDirection::Followers => (actor_id, r.actor_id, None, Some(other)),
            };
            Ok(MisskeyFollowRelation {
                id: r.follow_id.to_string(),
                created_at: r.created_at.to_rfc3339(),
                followee_id: followee_id.to_string(),
                follower_id: follower_id.to_string(),
                followee,
                follower,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()
        .map(Json)
}

/// POST /api/users/following — 指定ユーザーのフォロー中一覧（Misskey互換、#81）。
pub async fn users_following(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<UserCursorBody>,
) -> Result<Json<Vec<MisskeyFollowRelation>>, ApiError> {
    follow_relations(&state, me, body, FollowDirection::Following).await
}

/// POST /api/users/followers — 指定ユーザーのフォロワー一覧（Misskey互換、#81）。
pub async fn users_followers(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<UserCursorBody>,
) -> Result<Json<Vec<MisskeyFollowRelation>>, ApiError> {
    follow_relations(&state, me, body, FollowDirection::Followers).await
}

/// POST /api/notes/reactions — 指定リアクション種別を付けたユーザー一覧（Misskey互換、#81）。
/// Ariaで絵文字リアクションを長押しした際に呼ばれる。カスタムAPI
/// `GET /api/notes/:id/reactions/:content/actors`（`handlers::notes::reaction_actors`）と
/// 同じ `actors_for_reaction` を使う。投稿の可視性チェックも同様。
pub async fn notes_reactions(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<NotesReactionsBody>,
) -> Result<Json<Vec<MisskeyNoteReaction>>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);
    let note_id = parse_id(&body.note_id, "NOTE_NOT_FOUND")?;

    state
        .posts
        .find_by_id_for_viewer(note_id, my_actor_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("NOTE_NOT_FOUND"))?;

    let Some(reaction_type) = body.reaction_type else {
        return Ok(Json(vec![]));
    };
    let limit = body.limit.unwrap_or(10).clamp(1, 100);

    let actors = state
        .reactions
        .actors_for_reaction(note_id, &reaction_type, my_actor_id, limit)
        .await
        .map_err(internal)?;

    Ok(Json(
        actors
            .into_iter()
            .map(|a| MisskeyNoteReaction {
                id: a.reaction_id.to_string(),
                created_at: a.reaction_created_at.to_rfc3339(),
                user: user_lite(
                    ActorSummary {
                        id: a.id,
                        username: &a.username,
                        domain: &a.domain,
                        actor_type: &a.actor_type,
                        display_name: a.display_name.as_deref(),
                        avatar_url: a.avatar_url.as_deref(),
                    },
                    &state.local_domain,
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
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<UserCursorBody>,
) -> Result<Json<Vec<MisskeyUserReaction>>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);
    let actor_id = parse_id(&body.user_id, "USER_NOT_FOUND")?;
    let actor = state
        .actors
        .find_by_id(actor_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("USER_NOT_FOUND"))?;

    let rows = state
        .reactions
        .reactions_by_actor_for_feed(actor_id, my_actor_id, body.cursor.page(10), true)
        .await
        .map_err(internal)?;

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
    let (notes_count, users_count) = seiran_common::repository::post::local_stats(&state.db)
        .await
        .map_err(internal)?;

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
/// （Aria）。「リスト」は`users_lists_list`が実データを返すため対象外。
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

/// `ListRow`群を、メンバー一覧付き`MisskeyUserList`へ組み立てる（`users_lists_list`・
/// `users_lists_show`共通）。メンバーは全リスト分を1クエリで取得する。
async fn build_misskey_user_lists(
    state: &AppState,
    rows: Vec<seiran_common::repository::ListRow>,
) -> Result<Vec<MisskeyUserList>, ApiError> {
    let list_ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut members: HashMap<i64, Vec<String>> = HashMap::new();
    for (list_id, actor_id) in state
        .lists
        .member_ids_of_lists(&list_ids)
        .await
        .map_err(internal)?
    {
        members
            .entry(list_id)
            .or_default()
            .push(actor_id.to_string());
    }
    Ok(rows
        .into_iter()
        .map(|row| MisskeyUserList {
            id: row.id.to_string(),
            created_at: row.created_at.to_rfc3339(),
            user_ids: members.remove(&row.id).unwrap_or_default(),
            name: row.name,
            is_public: row.is_public,
        })
        .collect())
}

/// POST /api/users/lists/list — プロフィール「リスト」タブ・自分のリスト管理画面（Aria等）。
/// `userId`指定時はそのユーザーの公開リストのみ（本家Misskey準拠、プロフィール表示から
/// 他人のリストを覗く用途）、省略時は認証ユーザー自身の全リスト（非公開含む、自分のリスト
/// 管理画面用途）を返す。既存のカスタムAPI（`handlers::lists`）と同じ`ListRepository`を使う
/// （#251続き）。
pub async fn users_lists_list(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    body: Option<Json<UsersListsListBody>>,
) -> Result<Json<Vec<MisskeyUserList>>, ApiError> {
    let rows = match body.and_then(|Json(b)| b.user_id) {
        Some(uid) => {
            let actor_id = parse_id(&uid, "USER_NOT_FOUND")?;
            state.lists.list_public_by_owner(actor_id).await
        }
        None => {
            let me = me.ok_or(ApiError::Unauthorized("CREDENTIAL_REQUIRED"))?;
            state.lists.list_by_owner(me.actor_id).await
        }
    }
    .map_err(internal)?;
    Ok(Json(build_misskey_user_lists(&state, rows).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsersListsShowBody {
    pub list_id: String,
}

/// POST /api/users/lists/show — リストを開いた詳細画面（Aria等、`MisskeyUsersLists.show`）。
/// カスタムAPI `GET /api/lists/:id`
/// （`handlers::lists`）と同じ公開範囲チェック（非公開リストは所有者本人のみ、
/// `NO_SUCH_LIST`）を使う（#251続き）。
pub async fn users_lists_show(
    MaybeAuthedUser(me): MaybeAuthedUser,
    State(state): State<AppState>,
    Json(body): Json<UsersListsShowBody>,
) -> Result<Json<MisskeyUserList>, ApiError> {
    let my_actor_id = me.map(|u| u.actor_id);
    let list = find_viewable_list(&state, &body.list_id, my_actor_id).await?;
    build_misskey_user_lists(&state, vec![list])
        .await?
        .pop()
        .map(Json)
        .ok_or(ApiError::NotFound("NO_SUCH_LIST"))
}

#[cfg(test)]
mod tests {
    use super::CursorParams;

    fn cursor(limit: Option<i64>, since: Option<&str>, until: Option<&str>) -> CursorParams {
        CursorParams {
            limit,
            since_id: since.map(str::to_owned),
            until_id: until.map(str::to_owned),
        }
    }

    #[test]
    fn page_clamps_limit_and_uses_default() {
        assert_eq!(cursor(None, None, None).page(20).limit, 20);
        assert_eq!(cursor(Some(0), None, None).page(20).limit, 1);
        assert_eq!(cursor(Some(-5), None, None).page(20).limit, 1);
        assert_eq!(cursor(Some(1000), None, None).page(20).limit, 100);
    }

    #[test]
    fn page_ignores_non_numeric_ids() {
        let page = cursor(None, Some("abc"), Some("123")).page(10);
        assert_eq!(page.since_id, None);
        assert_eq!(page.until_id, Some(123));
    }

    #[test]
    fn cursor_params_deserialize_from_misskey_camel_case_body() {
        let body: super::TimelineBody =
            serde_json::from_value(serde_json::json!({"limit": 5, "untilId": "42"})).unwrap();
        let page = body.cursor.page(20);
        assert_eq!(
            (page.limit, page.until_id, page.since_id),
            (5, Some(42), None)
        );
    }
}
