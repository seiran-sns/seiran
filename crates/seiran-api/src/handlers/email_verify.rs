use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};

use seiran_common::generate_snowflake_id;

use crate::error::ApiError;
use crate::mailer::{send_verification_email, MailError};
use crate::AppState;

#[derive(Deserialize)]
pub struct VerifyEmailRequest {
    pub email: String,
}

#[derive(Serialize)]
pub struct VerifyEmailResponse {
    pub message: String,
}

#[derive(Deserialize)]
pub struct VerifyTokenQuery {
    pub token: String,
}

#[derive(Serialize)]
pub struct VerifyTokenResponse {
    /// このトークンを POST /api/auth/register に渡して登録を完了する
    pub registration_token: String,
}

/// Step 1: メールアドレスを受け取り確認メールを送信する
pub async fn request_email_verification(
    State(state): State<AppState>,
    Json(payload): Json<VerifyEmailRequest>,
) -> Result<Json<VerifyEmailResponse>, ApiError> {
    let email = payload.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::BadRequest("EMAIL_INVALID".into()));
    }

    // すでに登録済みのメールアドレスは拒否
    let exists = state
        .users
        .email_exists(&email)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if exists {
        return Err(ApiError::Conflict("EMAIL_ALREADY_REGISTERED"));
    }

    let pool = &state.db;
    let id = generate_snowflake_id(chrono::Utc::now());

    let row = sqlx::query!(
        "INSERT INTO email_verifications (id, email) VALUES ($1, $2) RETURNING token",
        id,
        email,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| ApiError::Internal(format!("DB エラー: {}", e)))?;

    let token = row.token;
    let verify_url = format!("https://{}/verify-email?token={}", state.local_domain, token);

    let smtp_settings = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    send_verification_email(&smtp_settings, &email, &verify_url)
        .await
        .map_err(|e| {
            eprintln!("[verify-email] メール送信失敗: {}", e);
            match e {
                MailError::Config(_) => ApiError::ServiceUnavailable("SMTP_NOT_CONFIGURED"),
                _ => ApiError::Internal(format!("メール送信失敗: {}", e)),
            }
        })?;

    Ok(Json(VerifyEmailResponse {
        message: format!("{} に確認メールを送信しました", email),
    }))
}

/// Step 2: 確認リンクのトークンを検証してクライアントに登録トークンを返す
/// トークンの消費は POST /api/auth/register の DELETE で行う（このエンドポイントは副作用なし）
pub async fn verify_email_token(
    Query(params): Query<VerifyTokenQuery>,
    State(state): State<AppState>,
) -> Result<Json<VerifyTokenResponse>, ApiError> {
    let pool = &state.db;

    let token: uuid::Uuid = params
        .token
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_TOKEN".into()))?;

    let row = sqlx::query!(
        "SELECT token FROM email_verifications
         WHERE token = $1
           AND expires_at > now()",
        token,
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::BadRequest("INVALID_TOKEN".into()))?;

    Ok(Json(VerifyTokenResponse {
        registration_token: row.token.to_string(),
    }))
}
