use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use seiran_common::generate_snowflake_id;

use crate::mailer::send_verification_email;
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
) -> impl IntoResponse {
    let email = payload.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return (StatusCode::BAD_REQUEST, "有効なメールアドレスを入力してください").into_response();
    }

    let pool = &state.db;
    let id = generate_snowflake_id(chrono::Utc::now());

    let row = match sqlx::query!(
        "INSERT INTO email_verifications (id, email) VALUES ($1, $2) RETURNING token",
        id,
        email,
    )
    .fetch_one(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[verify-email] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let token = row.token;
    let verify_url = format!("https://{}/auth/verify?token={}", state.local_domain, token);

    if let Err(e) = send_verification_email(&email, &verify_url).await {
        eprintln!("[verify-email] メール送信失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "メール送信に失敗しました").into_response();
    }

    Json(VerifyEmailResponse {
        message: format!("{} に確認メールを送信しました", email),
    })
    .into_response()
}

/// Step 2: 確認リンクを受け取り、verified_at を記録してクライアントに登録トークンを返す
pub async fn verify_email_token(
    Query(params): Query<VerifyTokenQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let token: Uuid = match params.token.parse() {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "無効なトークンです").into_response(),
    };

    let pool = &state.db;

    let row = sqlx::query!(
        "UPDATE email_verifications
         SET verified_at = now()
         WHERE token = $1
           AND verified_at IS NULL
           AND expires_at > now()
         RETURNING token",
        token,
    )
    .fetch_optional(pool)
    .await;

    match row {
        Ok(Some(r)) => Json(VerifyTokenResponse {
            registration_token: r.token.to_string(),
        })
        .into_response(),
        Ok(None) => (
            StatusCode::BAD_REQUEST,
            "トークンが無効か期限切れです",
        )
            .into_response(),
        Err(e) => {
            eprintln!("[verify-email] DB エラー: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response()
        }
    }
}
