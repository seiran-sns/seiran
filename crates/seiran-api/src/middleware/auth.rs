use axum::http::HeaderMap;
use seiran_common::{LocalAuthProvider, traits::AuthProvider};

use crate::error::ApiError;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub email: String,
}

pub async fn extract_auth(
    headers: &HeaderMap,
    auth: &LocalAuthProvider,
) -> Result<AuthUser, ApiError> {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized("Authorization ヘッダーが必要です"))?;

    let info = auth
        .verify_token(bearer)
        .await
        .map_err(|_| ApiError::Unauthorized("トークンが無効です"))?;

    let user_id: i64 = info
        .sub
        .strip_prefix("local|")
        .and_then(|s| s.parse().ok())
        .ok_or(ApiError::Unauthorized("トークン形式が不正です"))?;

    Ok(AuthUser { user_id, email: info.email })
}
