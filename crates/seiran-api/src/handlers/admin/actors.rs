//! 凍結済みアクター（ローカル・リモート共通）の管理画面向けAPI。
//!
//! `handlers::admin::users::{suspend_user, unsuspend_user}` はローカルユーザー管理画面
//! （user_id起点）向け、`handlers::admin::reports::suspend_subject` は通報対応画面向けだが、
//! いずれも最終的に `actors.suspended_at` を更新する。ここではその actor_id 起点の凍結/凍結解除と、
//! ローカル・リモート混在の「凍結済みユーザー」一覧タブ用の read model を提供する。

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::AppState;

use seiran_common::repository::actor::SuspendedActorRow;

#[derive(Debug, Serialize)]
pub struct SuspendedActorResponse {
    pub id: String,
    pub username: String,
    pub domain: String,
    pub actor_type: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub suspended_at: DateTime<Utc>,
    pub user_id: Option<String>,
    pub email: Option<String>,
}

impl From<SuspendedActorRow> for SuspendedActorResponse {
    fn from(r: SuspendedActorRow) -> Self {
        let avatar_url =
            seiran_common::avatar::resolve_avatar_url(r.avatar_url, &r.actor_type, &r.domain, r.id);
        Self {
            id: r.id.to_string(),
            username: r.username,
            domain: r.domain,
            actor_type: r.actor_type,
            display_name: r.display_name,
            avatar_url,
            suspended_at: r.suspended_at,
            user_id: r.user_id.map(|v| v.to_string()),
            email: r.email,
        }
    }
}

#[derive(Deserialize)]
pub struct ListSuspendedQuery {
    pub after_id: Option<String>,
    pub limit: Option<i64>,
}

/// GET /api/admin/suspended-actors?after_id=&limit=
///
/// ローカル・リモート混在の凍結済みアクター一覧（id昇順カーソルページネーション）。
pub async fn list_suspended(
    State(state): State<AppState>,
    Query(query): Query<ListSuspendedQuery>,
) -> Result<Json<Vec<SuspendedActorResponse>>, ApiError> {
    let after_id = query
        .after_id
        .as_deref()
        .map(|s| s.parse::<i64>())
        .transpose()
        .map_err(|_| ApiError::BadRequest("INVALID_AFTER_ID".to_owned()))?;
    let limit = query.limit.unwrap_or(30).clamp(1, 100);

    let rows = seiran_common::repository::actor::list_suspended(&state.db, after_id, limit)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// POST /api/admin/actors/:id/suspend
///
/// actor_id起点の凍結。ローカル・リモート共通（通報対応画面以外からも呼べる汎用版）。
pub async fn suspend_actor(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state
        .actors
        .set_suspended(id, true)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/admin/actors/:id/unsuspend
pub async fn unsuspend_actor(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state
        .actors
        .set_suspended(id, false)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
