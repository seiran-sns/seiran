use axum::http::HeaderMap;
use seiran_common::repository::{AppTokenRepository, UserRepository};
use seiran_common::LocalAuthProvider;

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
    users: &dyn UserRepository,
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

    // パスワード変更・リセットより前に発行されたトークンを一括拒否する
    // （token_valid_after が未設定なら制約なし）。
    if let Some(valid_after) = users
        .find_token_valid_after(verified.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        if (verified.iat as i64) < valid_after.timestamp() {
            return Err(ApiError::Unauthorized(
                "パスワード変更により無効化されたトークンです",
            ));
        }
    }

    Ok(AuthUser {
        user_id: verified.user_id,
        email: verified.email,
    })
}

/// JWT 検証 + role が `allowed` のいずれかであることをチェックする。
/// 該当しなければ 403 Forbidden を返す。
async fn require_role_in(
    headers: &HeaderMap,
    auth: &LocalAuthProvider,
    app_tokens: &dyn AppTokenRepository,
    users: &dyn UserRepository,
    allowed: &[&str],
) -> Result<AuthUser, ApiError> {
    let user = extract_auth(headers, auth, app_tokens, users).await?;
    let role = users
        .find_role_by_user_id(user.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .unwrap_or_default();
    if !allowed.contains(&role.as_str()) {
        return Err(ApiError::Forbidden("ADMIN_REQUIRED"));
    }
    Ok(user)
}

/// JWT 検証 + role = 'admin' チェック。
/// admin 以外は 403 Forbidden を返す。
pub async fn require_admin(
    headers: &HeaderMap,
    auth: &LocalAuthProvider,
    app_tokens: &dyn AppTokenRepository,
    users: &dyn UserRepository,
) -> Result<AuthUser, ApiError> {
    require_role_in(headers, auth, app_tokens, users, &["admin"]).await
}

/// JWT 検証 + 絵文字管理権限（admin / moderator / emoji-editor）チェック（#179）。
/// いずれでもなければ 403 Forbidden を返す。
pub async fn require_emoji_admin(
    headers: &HeaderMap,
    auth: &LocalAuthProvider,
    app_tokens: &dyn AppTokenRepository,
    users: &dyn UserRepository,
) -> Result<AuthUser, ApiError> {
    require_role_in(
        headers,
        auth,
        app_tokens,
        users,
        &["admin", "moderator", "emoji-editor"],
    )
    .await
}

/// JWT 検証 + 通報対応権限（admin / moderator）チェック（#179）。
/// moderator は調停者として通報の閲覧・対応（凍結・投稿削除・連合転送を含む）を行える。
/// いずれでもなければ 403 Forbidden を返す。
pub async fn require_report_moderator(
    headers: &HeaderMap,
    auth: &LocalAuthProvider,
    app_tokens: &dyn AppTokenRepository,
    users: &dyn UserRepository,
) -> Result<AuthUser, ApiError> {
    require_role_in(headers, auth, app_tokens, users, &["admin", "moderator"]).await
}
