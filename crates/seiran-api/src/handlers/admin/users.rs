use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use seiran_common::repository::AdminUserRow;

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::require_admin;

// ─── レスポンス DTO ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AdminUserResponse {
    pub id: String,
    pub email: String,
    pub role: String,
    pub suspended_at: Option<DateTime<Utc>>,
    pub username: Option<String>,
}

impl From<AdminUserRow> for AdminUserResponse {
    fn from(r: AdminUserRow) -> Self {
        Self {
            id: r.id.to_string(),
            email: r.email,
            role: r.role,
            suspended_at: r.suspended_at,
            username: r.username,
        }
    }
}

// ─── リクエスト DTO ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ChangeRoleRequest {
    pub role: String,
}

// ─── ハンドラ ─────────────────────────────────────────────────────────────

/// GET /api/admin/users
pub async fn list_users(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<AdminUserResponse>>, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    let rows = state
        .users
        .list_for_admin()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// POST /api/admin/users/:id/suspend
pub async fn suspend_user(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    state
        .users
        .set_suspended(id, true)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/admin/users/:id/unsuspend
pub async fn unsuspend_user(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    state
        .users
        .set_suspended(id, false)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/admin/users/:id/role
pub async fn change_user_role(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ChangeRoleRequest>,
) -> Result<StatusCode, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    if !matches!(req.role.as_str(), "user" | "moderator" | "admin") {
        return Err(ApiError::BadRequest("INVALID_ROLE".to_owned()));
    }

    state
        .users
        .update_role(id, req.role.as_str())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
