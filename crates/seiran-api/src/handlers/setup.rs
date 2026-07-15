use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use seiran_common::{generate_snowflake_id, LocalAuthProvider};
use seiran_common::atp::signing_key_from_pem;

use crate::AppState;
use crate::error::ApiError;
use crate::handlers::auth::{AuthResponse, UserInfo};

#[derive(Serialize)]
pub struct SetupStatus {
    pub initialized: bool,
}

#[derive(Deserialize)]
pub struct SetupRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

/// GET /api/setup/status
/// ユーザーが1件でも存在すれば initialized: true を返す。
pub async fn setup_status(
    State(state): State<AppState>,
) -> Result<Json<SetupStatus>, ApiError> {
    let count = state
        .users
        .count()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(SetupStatus { initialized: count > 0 }))
}

/// POST /api/setup
/// 初回セットアップ: 管理者ユーザーを作成する。
/// ユーザーが既に存在する場合は 409 を返す。メール確認は不要。
pub async fn setup(
    State(state): State<AppState>,
    Json(req): Json<SetupRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    if req.username.is_empty() || req.email.is_empty() || req.password.len() < 8 {
        return Err(ApiError::BadRequest("INVALID_INPUT".into()));
    }

    let count = state
        .users
        .count()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if count > 0 {
        return Err(ApiError::Conflict("ALREADY_INITIALIZED"));
    }

    let password_hash = LocalAuthProvider::hash_password(&req.password)
        .map_err(|e| {
            tracing::error!("[setup] ハッシュ失敗: {}", e);
            ApiError::Internal("パスワード処理エラー".to_string())
        })?;

    let rotation_key = signing_key_from_pem(&state.secrets.atproto_private_key_pem)
        .map_err(|e| {
            tracing::error!("[setup] 回転鍵ロード失敗: {}", e);
            ApiError::Internal("ATP鍵ロードエラー".to_string())
        })?;

    // DID確定 → TXT セット → PLC送信（最大3回リトライ）。成功後に DB 書き込み
    // （失敗時はロールバック不要、DB 未書き込みのため）。
    let (at_did, at_signing_key_pem, cf_record_id) =
        crate::handlers::plc_genesis::register_plc_did(&state, &req.username, &rotation_key, "setup").await?;

    let user_id = state
        .users
        .insert(&req.email, &password_hash, "admin")
        .await
        .map_err(|e| {
            tracing::error!("[setup] users INSERT 失敗: {}", e);
            ApiError::Internal("ユーザー作成エラー".to_string())
        })?;

    let actor_id = generate_snowflake_id(chrono::Utc::now());
    state
        .actors
        .insert_local(
            actor_id,
            user_id,
            &req.username,
            &state.local_domain,
            &at_did,
            &at_signing_key_pem,
        )
        .await
        .map_err(|e| {
            tracing::error!("[setup] actors INSERT 失敗: {}", e);
            ApiError::Internal("アクター作成エラー".to_string())
        })?;

    let now = chrono::Utc::now();
    if let Err(e) = state.atp_service.commit_profile(actor_id, &req.username, None, None, None, now).await {
        tracing::error!("[setup] ATP プロフィールコミット失敗（登録は完了済み）: {}", e);
    }

    let _ = cf_record_id;

    let token = state.local_auth.generate_token(user_id, &req.email)
        .map_err(|e| {
            tracing::error!("[setup] JWT 生成失敗: {}", e);
            ApiError::Internal("トークン生成エラー".to_string())
        })?;

    Ok(Json(AuthResponse {
        token,
        user: UserInfo {
            id: user_id,
            username: req.username,
            email: req.email,
            role: "admin".to_string(),
            actor_id,
            avatar_url: None, // セットアップ直後はアバター未設定
        },
    }))
}
