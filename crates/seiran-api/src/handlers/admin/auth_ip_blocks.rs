//! 認証ブルートフォース対策で自動ブロックされたIPアドレスの管理画面向け一覧・解除（#223）。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;

use seiran_common::repository::IpBlockRow;

use crate::error::ApiError;
use crate::AppState;

#[derive(Serialize)]
pub struct IpBlockResponse {
    pub ip_address: String,
    pub blocked_until: DateTime<Utc>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

impl From<IpBlockRow> for IpBlockResponse {
    fn from(r: IpBlockRow) -> Self {
        Self {
            ip_address: r.ip_address,
            blocked_until: r.blocked_until,
            reason: r.reason,
            created_at: r.created_at,
        }
    }
}

/// GET /api/admin/auth-ip-blocks
pub async fn list_ip_blocks(
    State(state): State<AppState>,
) -> Result<Json<Vec<IpBlockResponse>>, ApiError> {
    let rows = state
        .auth_rate_limits
        .list_active_ip_blocks()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// DELETE /api/admin/auth-ip-blocks/:ip
pub async fn unblock_ip(
    State(state): State<AppState>,
    Path(ip): Path<String>,
) -> Result<StatusCode, ApiError> {
    let removed = state
        .auth_rate_limits
        .unblock_ip(&ip)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !removed {
        return Err(ApiError::NotFound("IP_BLOCK_NOT_FOUND"));
    }
    Ok(StatusCode::NO_CONTENT)
}
