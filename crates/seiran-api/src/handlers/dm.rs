//! ダイレクトメッセージ（DM）ハンドラ。
//!
//! DM本体（`visibility='direct'`投稿の作成）は既存の `POST /api/notes` を再利用する
//! （`handlers::notes::create_note`）。ここではDM専用の一覧・履歴・既読状態のみを扱う。

use std::collections::{HashMap, HashSet};

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use crate::error::ApiError;
use crate::middleware::AuthedUser;
use crate::AppState;

use super::notes::dto::{to_note_response, NoteResponse, TimelineQuery};
use super::notes::{fetch_attachments_map, fetch_reactions_map, resolve_mention_facets_in_place};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DmPeerResponse {
    pub id: String,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub actor_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DmSessionResponse {
    pub thread_root_post_id: String,
    pub last_message: NoteResponse,
    pub peers: Vec<DmPeerResponse>,
    pub unread: bool,
}

/// `GET /api/dm/sessions` — 自分が参加しているDMセッション一覧（最終メッセージ順）。
pub async fn sessions(
    Query(q): Query<TimelineQuery>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = user.actor_id;
    let limit = q.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let summaries = match state.dm.sessions(actor_id, limit, until_id, since_id).await {
        Ok(s) => s,
        Err(e) => return ApiError::Internal(format!("DMセッション取得失敗: {}", e)).into_response(),
    };
    if summaries.is_empty() {
        return Json(Vec::<DmSessionResponse>::new()).into_response();
    }

    let thread_root_ids: Vec<i64> = summaries.iter().map(|s| s.thread_root_post_id).collect();
    let read_states: HashMap<i64, i64> = match state.dm.read_states(actor_id, &thread_root_ids).await {
        Ok(rows) => rows.into_iter().collect(),
        Err(e) => return ApiError::Internal(format!("既読状態取得失敗: {}", e)).into_response(),
    };

    let all_peer_ids: Vec<i64> = summaries
        .iter()
        .flat_map(|s| s.peer_actor_ids.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let peer_summaries = match state.dm.peer_summaries(&all_peer_ids).await {
        Ok(p) => p,
        Err(e) => return ApiError::Internal(format!("宛先アクター取得失敗: {}", e)).into_response(),
    };
    let peer_map: HashMap<i64, DmPeerResponse> = peer_summaries
        .into_iter()
        .map(|p| {
            (
                p.id,
                DmPeerResponse {
                    id: p.id.to_string(),
                    username: p.username,
                    domain: p.domain,
                    display_name: p.display_name,
                    actor_type: p.actor_type,
                    avatar_url: p.avatar_url,
                },
            )
        })
        .collect();

    // 最終メッセージ本体をNoteResponse形式で組み立てる（既存のタイムライン変換と共通の経路）。
    let last_post_ids: Vec<i64> = summaries.iter().map(|s| s.last_post_id).collect();
    let mut last_posts = match state.posts.find_by_ids(&last_post_ids).await {
        Ok(rows) => rows,
        Err(e) => return ApiError::Internal(format!("最終メッセージ取得失敗: {}", e)).into_response(),
    };
    resolve_mention_facets_in_place(&state.db, &mut last_posts).await;
    let mut att_map = fetch_attachments_map(&state.db, &last_post_ids).await;
    let rmap = fetch_reactions_map(&state.db, &last_post_ids, Some(actor_id)).await;
    let mut last_post_by_id: HashMap<i64, NoteResponse> = last_posts
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(p, att_map.remove(&id).unwrap_or_default());
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            (id, nr)
        })
        .collect();

    let result: Vec<DmSessionResponse> = summaries
        .into_iter()
        .filter_map(|s| {
            let last_message = last_post_by_id.remove(&s.last_post_id)?;
            let peers: Vec<DmPeerResponse> = s
                .peer_actor_ids
                .iter()
                .filter_map(|id| peer_map.get(id).cloned())
                .collect();
            let last_read = read_states.get(&s.thread_root_post_id).copied().unwrap_or(0);
            Some(DmSessionResponse {
                thread_root_post_id: s.thread_root_post_id.to_string(),
                unread: s.last_post_id > last_read,
                last_message,
                peers,
            })
        })
        .collect();

    Json(result).into_response()
}

/// `GET /api/dm/sessions/:thread_root_id/messages` — メッセージ履歴（時刻順、最下部が最新）。
pub async fn thread_messages(
    Path(thread_root_id): Path<i64>,
    Query(q): Query<TimelineQuery>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> Response {
    let actor_id = user.actor_id;
    match state.dm.is_participant(thread_root_id, actor_id).await {
        Ok(true) => {}
        Ok(false) => return ApiError::Forbidden("DM_NOT_PARTICIPANT").into_response(),
        Err(e) => return ApiError::Internal(format!("参加者チェック失敗: {}", e)).into_response(),
    }

    let limit = q.limit.unwrap_or(50).min(200);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let mut rows = match state.dm.thread_messages(thread_root_id, limit, until_id, since_id).await {
        Ok(r) => r,
        Err(e) => return ApiError::Internal(format!("メッセージ履歴取得失敗: {}", e)).into_response(),
    };
    resolve_mention_facets_in_place(&state.db, &mut rows).await;
    let ids: Vec<i64> = rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &ids).await;
    let rmap = fetch_reactions_map(&state.db, &ids, Some(actor_id)).await;
    let notes: Vec<NoteResponse> = rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(p, att_map.remove(&id).unwrap_or_default());
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            nr
        })
        .collect();
    Json(notes).into_response()
}

/// `POST /api/dm/sessions/:thread_root_id/read` — 既読状態を更新する。
pub async fn mark_read(
    Path(thread_root_id): Path<i64>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> Response {
    let actor_id = user.actor_id;
    match state.dm.is_participant(thread_root_id, actor_id).await {
        Ok(true) => {}
        Ok(false) => return ApiError::Forbidden("DM_NOT_PARTICIPANT").into_response(),
        Err(e) => return ApiError::Internal(format!("参加者チェック失敗: {}", e)).into_response(),
    }

    let latest = match state.dm.latest_post_id(thread_root_id).await {
        Ok(v) => v,
        Err(e) => return ApiError::Internal(format!("最新ポスト取得失敗: {}", e)).into_response(),
    };
    let Some(last_post_id) = latest else {
        return Json(serde_json::json!({"ok": true})).into_response();
    };

    match state.dm.mark_read(actor_id, thread_root_id, last_post_id).await {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => ApiError::Internal(format!("既読更新失敗: {}", e)).into_response(),
    }
}

#[derive(Serialize)]
pub struct UnreadCountResponse {
    pub count: i64,
}

/// `GET /api/dm/unread-count` — 未読のあるセッション数（左ペインバッジ用）。
pub async fn unread_count(user: AuthedUser, State(state): State<AppState>) -> Response {
    match state.dm.unread_session_count(user.actor_id).await {
        Ok(count) => Json(UnreadCountResponse { count }).into_response(),
        Err(e) => ApiError::Internal(format!("未読数取得失敗: {}", e)).into_response(),
    }
}
