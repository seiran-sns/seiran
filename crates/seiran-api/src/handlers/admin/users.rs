use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use seiran_common::repository::AdminUserRow;

use crate::error::ApiError;
use crate::middleware::require_admin;
use crate::AppState;

// ─── レスポンス DTO ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AdminUserResponse {
    pub id: String,
    pub email: String,
    pub role: String,
    pub suspended_at: Option<DateTime<Utc>>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub totp_enabled: bool,
    pub passkey_count: i64,
    /// 表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（#186）。
    pub emojis: std::collections::HashMap<String, String>,
}

impl From<AdminUserRow> for AdminUserResponse {
    fn from(r: AdminUserRow) -> Self {
        Self {
            id: r.id.to_string(),
            email: r.email,
            role: r.role,
            suspended_at: r.suspended_at,
            username: r.username,
            display_name: r.display_name,
            avatar_url: r.avatar_url,
            totp_enabled: r.totp_enabled,
            passkey_count: r.passkey_count,
            emojis: r
                .emoji_map
                .as_ref()
                .map(crate::handlers::users::json_map_to_string_map)
                .unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
pub struct ListUsersQuery {
    pub q: Option<String>,
    pub after_id: Option<String>,
    pub limit: Option<i64>,
}

// ─── リクエスト DTO ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ChangeRoleRequest {
    pub role: String,
}

// ─── ハンドラ ─────────────────────────────────────────────────────────────

/// GET /api/admin/users?q=...&after_id=...&limit=...
///
/// `q`: 表示名/ユーザー名/メールアドレスの部分一致絞り込み（省略可）。
/// `after_id`: 無限スクロール用カーソル。この ID より大きい行から返す（省略可）。
/// `limit`: 1ページの件数（省略時30、最大100）。
pub async fn list_users(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<Vec<AdminUserResponse>>, ApiError> {
    require_admin(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    let after_id = query
        .after_id
        .as_deref()
        .map(|s| s.parse::<i64>())
        .transpose()
        .map_err(|_| ApiError::BadRequest("INVALID_AFTER_ID".to_owned()))?;
    let limit = query.limit.unwrap_or(30).clamp(1, 100);

    let rows = state
        .users
        .list_for_admin(query.q.as_deref(), after_id, limit)
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
    require_admin(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

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
    require_admin(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

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
    require_admin(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    if !matches!(
        req.role.as_str(),
        "user" | "moderator" | "emoji-editor" | "admin"
    ) {
        return Err(ApiError::BadRequest("INVALID_ROLE".to_owned()));
    }

    state
        .users
        .update_role(id, req.role.as_str())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/admin/users/:id/totp/disable
///
/// 管理者が、認証手段を失ったユーザーのTOTP設定を強制解除する。
/// `user_totp` の削除によりリカバリーコードとメール解除要求もCASCADE削除される。
pub async fn disable_user_totp(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_admin(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    state
        .totp
        .delete(id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}
