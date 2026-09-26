//! ハッシュタグ機能のAPIハンドラ。
//!
//! ハッシュタグはポストとm:nの関係を持つ永続化オブジェクト（`docs/database.md` 参照）。
//! ローカル投稿・AP受信・Bsky受信いずれの投稿からも同じ `hashtags`/`post_hashtags` に
//! 集約されるため、ここで返すタイムラインは出自を問わず同列に扱う。

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;

use crate::error::ApiError;
use crate::handlers::notes::build_note_responses;
use crate::handlers::notes::dto::TimelineQuery;
use crate::middleware::{AuthedUser, MaybeAuthedUser};
use crate::AppState;

#[derive(Serialize)]
pub struct PinnedHashtagResponse {
    pub name: String,
}

/// クライアントから届く `:name` を正規化する（先頭 `#` 除去・小文字化）。
/// 素の "foo" でも "#foo" でもリクエストできるようにする。
fn normalize_tag_name(raw: &str) -> String {
    raw.trim_start_matches('#').to_lowercase()
}

pub async fn hashtag_timeline(
    Path(name): Path<String>,
    Query(q): Query<TimelineQuery>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let tag_name = normalize_tag_name(&name);
    if tag_name.is_empty() {
        return ApiError::BadRequest("不正なハッシュタグです".to_string()).into_response();
    }

    let viewer_actor_id = user.as_ref().map(|u| u.actor_id);
    let limit = q.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match state
        .hashtags
        .timeline(&tag_name, limit, until_id, since_id, viewer_actor_id)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[hashtag_timeline] クエリ失敗: {}", e);
            return ApiError::Internal("TL取得に失敗しました".to_string()).into_response();
        }
    };
    let notes = build_note_responses(&state, rows, viewer_actor_id).await;
    Json(notes).into_response()
}

pub async fn pinned_hashtags(user: AuthedUser, State(state): State<AppState>) -> impl IntoResponse {
    match state.hashtags.list_pinned(user.actor_id).await {
        Ok(rows) => {
            let out: Vec<PinnedHashtagResponse> = rows
                .into_iter()
                .map(|r| PinnedHashtagResponse { name: r.name })
                .collect();
            Json(out).into_response()
        }
        Err(e) => {
            ApiError::Internal(format!("ピン留めハッシュタグ取得失敗: {}", e)).into_response()
        }
    }
}

pub async fn pin_hashtag(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let tag_name = normalize_tag_name(&name);
    if tag_name.is_empty() {
        return ApiError::BadRequest("不正なハッシュタグです".to_string()).into_response();
    }
    match state
        .hashtags
        .pin(user.actor_id, &tag_name, chrono::Utc::now())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::Internal(format!("ハッシュタグのピン留め失敗: {}", e)).into_response(),
    }
}

pub async fn unpin_hashtag(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let tag_name = normalize_tag_name(&name);
    match state.hashtags.unpin(user.actor_id, &tag_name).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            ApiError::Internal(format!("ハッシュタグのピン留め解除失敗: {}", e)).into_response()
        }
    }
}
