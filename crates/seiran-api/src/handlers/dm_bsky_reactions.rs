//! bsky宛DMメッセージの絵文字リアクション付与/取消・「隠す」（自分の画面からだけ非表示）。
//!
//! 通常投稿の`reactions`テーブル（1投稿1ユーザー1個まで）とは異なり、Bluesky公式チャット
//! API（`chat.bsky.convo.addReaction`）は「1ユーザーが複数の異なる絵文字を同一メッセージに
//! 付けられる（メッセージ全体で最大5件、同じ絵文字は1個まで）」という仕様のため、専用の
//! ハンドラ・データモデル（`dm_bsky_reactions`）で扱う。fedi/localのDMリアクションは
//! 通常投稿と同じ`handlers::notes::reactions::create_reaction`等をそのまま使う
//! （`docs/protocols.md` 9節参照）。

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};

use crate::error::ApiError;
use crate::middleware::AuthedUser;
use crate::AppState;

use super::notes::dto::ReactRequest;
use super::notes::fetch_dm_bsky_reactions_map;
use super::notes::validation::{validate_reaction_content, ReactionContent};

/// Bluesky公式チャットの`chat.bsky.convo.addReaction`仕様上の上限（メッセージ全体、
/// 全ユーザー合計）。
const MAX_BSKY_DM_REACTIONS_PER_MESSAGE: i64 = 5;

/// POST /api/dm/messages/:id/reactions
pub async fn create_dm_bsky_reaction(
    Path(post_id_str): Path<String>,
    me: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<ReactRequest>,
) -> Response {
    let post_id: i64 = match post_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    // 可視性チェック（DM参加者本人かどうか）。
    match state.posts.find_by_id_for_viewer(post_id, Some(me.actor_id)).await {
        Ok(Some(_)) => {}
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    }

    if state
        .dm
        .bsky_dm_context(post_id, me.actor_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return ApiError::BadRequest("NOT_BSKY_DM".to_owned()).into_response();
    }

    // Bsky仕様上Unicode絵文字のみ（カスタム絵文字非対応）。
    let content = match validate_reaction_content(&req.content) {
        Ok(ReactionContent::Unicode(u)) => u,
        Ok(ReactionContent::Custom(_)) => {
            return ApiError::BadRequest("BSKY_REACTION_UNICODE_ONLY".to_owned()).into_response()
        }
        Err(e) => return e.into_response(),
    };

    match state.dm.count_bsky_reactions(post_id).await {
        Ok(count) if count >= MAX_BSKY_DM_REACTIONS_PER_MESSAGE => {
            return ApiError::BadRequest("BSKY_REACTION_LIMIT_EXCEEDED".to_owned()).into_response();
        }
        Ok(_) => {}
        Err(e) => {
            return ApiError::Internal(format!("リアクション数取得失敗: {}", e)).into_response()
        }
    }

    let id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    let inserted = match state
        .dm
        .add_bsky_reaction(id, post_id, me.actor_id, &content, chrono::Utc::now())
        .await
    {
        Ok(v) => v,
        Err(e) => return ApiError::Internal(format!("リアクション保存失敗: {}", e)).into_response(),
    };
    if !inserted {
        // 同じ絵文字は1個まで（Bsky仕様、UNIQUE(post_id, actor_id, content)）。
        return ApiError::BadRequest("BSKY_REACTION_ALREADY_EXISTS".to_owned()).into_response();
    }
    state
        .enqueue_bsky_dm_reaction_add(post_id, me.actor_id, content)
        .await;

    let rmap = fetch_dm_bsky_reactions_map(&state.db, &[post_id], Some(me.actor_id)).await;
    Json(serde_json::json!({
        "ok": true,
        "reactions": rmap.get(&post_id).cloned().unwrap_or_default(),
    }))
    .into_response()
}

/// DELETE /api/dm/messages/:id/reactions/:content
pub async fn delete_dm_bsky_reaction(
    Path((post_id_str, content)): Path<(String, String)>,
    me: AuthedUser,
    State(state): State<AppState>,
) -> Response {
    let post_id: i64 = match post_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    match state.posts.find_by_id_for_viewer(post_id, Some(me.actor_id)).await {
        Ok(Some(_)) => {}
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    }

    let deleted = match state
        .dm
        .remove_bsky_reaction(post_id, me.actor_id, &content)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            return ApiError::Internal(format!("リアクション削除失敗: {}", e)).into_response()
        }
    };
    if deleted == 0 {
        return ApiError::NotFound("REACTION_NOT_FOUND").into_response();
    }
    state
        .enqueue_bsky_dm_reaction_remove(post_id, me.actor_id, content)
        .await;

    let rmap = fetch_dm_bsky_reactions_map(&state.db, &[post_id], Some(me.actor_id)).await;
    Json(serde_json::json!({
        "ok": true,
        "reactions": rmap.get(&post_id).cloned().unwrap_or_default(),
    }))
    .into_response()
}

/// POST /api/dm/messages/:id/hide
/// 自分の画面からだけメッセージを非表示にする（`chat.bsky.convo.deleteMessageForSelf`
/// 相当）。Bsky DMは相手側からメッセージを削除できない仕様のため、fedi/local向けの
/// 完全削除（`DELETE /api/notes/:id`）とは別に用意する。bsky宛でないメッセージにも
/// 呼べる（自分の画面を整理したいだけの用途もありうるため拒否しない）。
pub async fn hide_dm_message(Path(post_id_str): Path<String>, me: AuthedUser, State(state): State<AppState>) -> Response {
    let post_id: i64 = match post_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    match state.posts.find_by_id_for_viewer(post_id, Some(me.actor_id)).await {
        Ok(Some(_)) => {}
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    }

    if let Err(e) = state.dm.hide_message(me.actor_id, post_id).await {
        return ApiError::Internal(format!("非表示設定失敗: {}", e)).into_response();
    }

    if state
        .dm
        .bsky_dm_context(post_id, me.actor_id)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        state.enqueue_bsky_dm_hide(post_id, me.actor_id).await;
    }

    Json(serde_json::json!({"ok": true})).into_response()
}
