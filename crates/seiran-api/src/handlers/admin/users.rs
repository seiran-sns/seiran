use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::require_admin;

// ─── DB 行 ────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct UserRow {
    id: i64,
    email: String,
    role: String,
    suspended_at: Option<DateTime<Utc>>,
    username: Option<String>,
}

// ─── レスポンス DTO ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AdminUserResponse {
    pub id: String,
    pub email: String,
    pub role: String,
    pub suspended_at: Option<DateTime<Utc>>,
    pub username: Option<String>,
}

impl From<UserRow> for AdminUserResponse {
    fn from(r: UserRow) -> Self {
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

    let rows: Vec<UserRow> = sqlx::query_as::<_, UserRow>(
        "SELECT u.id, u.email, u.role::text AS role, u.suspended_at, a.username
         FROM users u
         LEFT JOIN actors a ON a.user_id = u.id AND a.actor_type::text = 'local'
         ORDER BY u.id
         LIMIT 100",
    )
    .fetch_all(&state.db)
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

    sqlx::query("UPDATE users SET suspended_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(&state.db)
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

    sqlx::query("UPDATE users SET suspended_at = NULL WHERE id = $1")
        .bind(id)
        .execute(&state.db)
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

    sqlx::query("UPDATE users SET role = $1::user_role WHERE id = $2")
        .bind(req.role.as_str())
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
