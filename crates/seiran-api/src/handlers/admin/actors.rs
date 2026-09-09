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

#[derive(Debug, sqlx::FromRow)]
struct SuspendedActorRow {
    id: i64,
    username: String,
    domain: String,
    actor_type: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
    suspended_at: DateTime<Utc>,
    /// ローカルアクターの場合のみ `Some`（管理画面からユーザー管理タブへ辿るため）。
    user_id: Option<i64>,
    email: Option<String>,
}

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

    let rows = sqlx::query_as::<_, SuspendedActorRow>(
        "SELECT a.id, a.username, a.domain, a.actor_type::text AS actor_type, a.display_name,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url,
                a.suspended_at, a.user_id, u.email
         FROM actors a
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         LEFT JOIN users u ON u.id = a.user_id
         WHERE a.suspended_at IS NOT NULL AND ($1::bigint IS NULL OR a.id > $1)
         ORDER BY a.id
         LIMIT $2",
    )
    .bind(after_id)
    .bind(limit)
    .fetch_all(&state.db)
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
