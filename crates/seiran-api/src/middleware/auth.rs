use axum::http::HeaderMap;
use seiran_common::{LocalAuthProvider};
use seiran_common::repository::{AppTokenRepository, UserRepository};

use crate::error::ApiError;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub email: String,
}

pub async fn extract_auth(
    headers: &HeaderMap,
    auth: &LocalAuthProvider,
    app_tokens: &dyn AppTokenRepository,
) -> Result<AuthUser, ApiError> {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized("Authorization ヘッダーが必要です"))?;

    let verified = auth
        .verify_token(bearer)
        .map_err(|_| ApiError::Unauthorized("トークンが無効です"))?;

    // #60: MiAuth 発行分は app_tokens に記録され、無効化されていれば拒否する
    // （記録が無い jti は自社ログイン等の管理対象外トークンとして常に有効）。
    let revoked = app_tokens
        .is_revoked(verified.jti)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if revoked {
        return Err(ApiError::Unauthorized("トークンが無効化されています"));
    }

    Ok(AuthUser { user_id: verified.user_id, email: verified.email })
}

/// JWT 検証 + role = 'admin' チェック。
/// admin 以外は 403 Forbidden を返す。
pub async fn require_admin(
    headers: &HeaderMap,
    auth: &LocalAuthProvider,
    app_tokens: &dyn AppTokenRepository,
    users: &dyn UserRepository,
) -> Result<AuthUser, ApiError> {
    let user = extract_auth(headers, auth, app_tokens).await?;
    let role = users
        .find_role_by_user_id(user.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .unwrap_or_default();
    if role != "admin" {
        return Err(ApiError::Forbidden("ADMIN_REQUIRED"));
    }
    Ok(user)
}
