//! TOTP（二段階認証）: 設定画面での有効化・無効化、ログイン時の検証、
//! メール経由の強制解除（#65）。

use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};

use seiran_common::{crypto, generate_snowflake_id, totp as totp_util, LocalAuthProvider};

use crate::handlers::auth::{finish_login, AuthResponse};
use crate::mailer::send_totp_disable_email;
use crate::rate_limit::{self, AttemptKind};
use crate::{
    error::ApiError,
    middleware::{extract_auth, ClientIp},
    AppState,
};

const RECOVERY_CODE_COUNT: usize = 10;

#[derive(Serialize)]
pub struct TotpStatusResponse {
    pub enabled: bool,
}

/// `GET /api/account/totp/status`
pub async fn totp_status(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<TotpStatusResponse>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
    let enabled = state
        .totp
        .find_by_user_id(auth_user.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map(|(_, enabled)| enabled)
        .unwrap_or(false);
    Ok(Json(TotpStatusResponse { enabled }))
}

async fn find_username(state: &AppState, user_id: i64) -> Result<String, ApiError> {
    sqlx::query_scalar!(
        "SELECT a.username FROM actors a WHERE a.user_id = $1 AND a.actor_type::text = 'local'",
        user_id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::BadRequest("USER_NOT_FOUND".to_owned()))
}

fn decrypt_secret(state: &AppState, secret_encrypted: &str) -> Result<String, ApiError> {
    crypto::decrypt(secret_encrypted, &state.secrets.encryption_key_bytes())
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or_else(|| ApiError::Internal("[totp] シークレット復号失敗".to_owned()))
}

#[derive(Serialize)]
pub struct TotpSetupResponse {
    pub secret: String,
    pub otpauth_url: String,
}

/// `POST /api/account/totp/setup`
/// シークレットを新規生成し（enabled=falseの未確定状態で）保存する。
/// 確認コードの検証・有効化確定は`totp_enable`で行う。
pub async fn totp_setup(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<TotpSetupResponse>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    if let Some((_, true)) = state
        .totp
        .find_by_user_id(auth_user.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        return Err(ApiError::Conflict("TOTP_ALREADY_ENABLED"));
    }

    let username = find_username(&state, auth_user.user_id).await?;

    let secret = totp_util::generate_secret_base32();
    let secret_encrypted =
        crypto::encrypt(secret.as_bytes(), &state.secrets.encryption_key_bytes())
            .map_err(|e| ApiError::Internal(format!("[totp-setup] 暗号化失敗: {:?}", e)))?;

    let id = generate_snowflake_id(chrono::Utc::now());
    state
        .totp
        .upsert_pending(id, auth_user.user_id, &secret_encrypted)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let otpauth_url = totp_util::build_otpauth_url(&secret, &username)
        .map_err(|e| ApiError::Internal(format!("[totp-setup] otpauth url 生成失敗: {}", e)))?;

    Ok(Json(TotpSetupResponse {
        secret,
        otpauth_url,
    }))
}

#[derive(Deserialize)]
pub struct TotpEnableRequest {
    pub code: String,
}

#[derive(Serialize)]
pub struct TotpEnableResponse {
    pub recovery_codes: Vec<String>,
}

/// `POST /api/account/totp/enable`
/// `totp_setup`で保存したシークレットに対する確認コードを検証し、成功したら有効化 +
/// リカバリーコードを新規発行する。
pub async fn totp_enable(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<TotpEnableRequest>,
) -> Result<Json<TotpEnableResponse>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    let (secret_encrypted, enabled) = state
        .totp
        .find_by_user_id(auth_user.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::BadRequest("TOTP_NOT_SETUP".to_owned()))?;
    if enabled {
        return Err(ApiError::Conflict("TOTP_ALREADY_ENABLED"));
    }

    let secret = decrypt_secret(&state, &secret_encrypted)?;
    let username = find_username(&state, auth_user.user_id).await?;

    if !totp_util::verify_code(&secret, &username, &req.code) {
        return Err(ApiError::BadRequest("TOTP_CODE_INVALID".to_owned()));
    }

    // リカバリーコードを新規発行する（平文はこのレスポンスで一度だけ返す。
    // DBにはArgon2ハッシュのみ保存する＝password_hashと同方式）。
    let mut plain_codes = Vec::with_capacity(RECOVERY_CODE_COUNT);
    let mut rows = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for _ in 0..RECOVERY_CODE_COUNT {
        let code = totp_util::generate_recovery_code();
        let hash = LocalAuthProvider::hash_password(&code).map_err(|e| {
            ApiError::Internal(format!("[totp-enable] リカバリーコードハッシュ失敗: {}", e))
        })?;
        let id = generate_snowflake_id(chrono::Utc::now());
        rows.push((id, auth_user.user_id, hash));
        plain_codes.push(code);
    }
    state
        .totp
        .enable_with_recovery_codes(auth_user.user_id, &rows)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(TotpEnableResponse {
        recovery_codes: plain_codes,
    }))
}

#[derive(Deserialize)]
pub struct TotpDisableRequest {
    pub current_password: String,
}

/// `POST /api/account/totp/disable`
/// ログイン中（TOTP検証済み）ユーザーが設定画面から自分で無効化する経路。
/// パスワード確認のみを要求する（変更系の他エンドポイントと同水準）。
pub async fn totp_disable(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<TotpDisableRequest>,
) -> Result<Json<()>, ApiError> {
    let auth_user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    let hash = sqlx::query_scalar!(
        "SELECT password_hash FROM users WHERE id = $1",
        auth_user.user_id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .flatten()
    .ok_or(ApiError::BadRequest("USER_NOT_FOUND".to_owned()))?;

    let ok = LocalAuthProvider::verify_password(&req.current_password, &hash)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !ok {
        return Err(ApiError::BadRequest(
            "CURRENT_PASSWORD_INCORRECT".to_owned(),
        ));
    }

    state
        .totp
        .delete(auth_user.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(()))
}

#[derive(Deserialize)]
pub struct TotpVerifyRequest {
    pub pending_token: String,
    pub code: String,
}

/// `POST /api/auth/totp/verify`
/// ログイン2段階目。`code`は6桁のTOTPコード、または`nnnn-nnnn`形式のリカバリーコード。
pub async fn totp_verify(
    State(state): State<AppState>,
    ip: ClientIp,
    Json(req): Json<TotpVerifyRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    rate_limit::check_ip_not_blocked(&state, &ip).await?;

    let user_id = state
        .local_auth
        .verify_pending_totp_token(&req.pending_token)
        .map_err(|_| ApiError::Unauthorized("INVALID_PENDING_TOKEN"))?;

    let (secret_encrypted, enabled) = state
        .totp
        .find_by_user_id(user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::Unauthorized("TOTP_NOT_ENABLED"))?;
    if !enabled {
        return Err(ApiError::Unauthorized("TOTP_NOT_ENABLED"));
    }

    let row = sqlx::query!(
        "SELECT u.email, a.username FROM users u
         JOIN actors a ON a.user_id = u.id AND a.actor_type::text = 'local'
         WHERE u.id = $1",
        user_id
    )
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::Unauthorized("INVALID_CREDENTIALS"))?;

    // ログインと同じ「試行種類数」ベースのブルートフォース対策をTOTPコード/リカバリー
    // コードにも適用する（issue #223: パラメータ共用）。
    let last_reset_at = state
        .password_resets
        .find_last_used_at(user_id)
        .await
        .ok()
        .flatten();
    rate_limit::check_and_record_credential_attempt(
        &state,
        AttemptKind::Totp,
        &ip,
        &row.username,
        &req.code,
        last_reset_at,
    )
    .await?;

    let code_trimmed = req.code.trim();
    // 6桁の数字のみ = TOTPコード、それ以外（"nnnn-nnnn"形式含む）はリカバリーコードとして扱う。
    let is_totp_format =
        code_trimmed.len() == 6 && code_trimmed.chars().all(|c| c.is_ascii_digit());

    let verified = if is_totp_format {
        let secret = decrypt_secret(&state, &secret_encrypted)?;
        totp_util::verify_code(&secret, &row.username, code_trimmed)
    } else {
        // リカバリーコード照合: Argon2ハッシュは非決定的なため、未使用コードを
        // 全件取得して1件ずつ照合する（1ユーザー最大10件なのでコスト上問題ない）。
        let candidates = state
            .totp
            .list_unused_recovery_codes(user_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        let mut matched_id = None;
        for (id, hash) in candidates {
            if LocalAuthProvider::verify_password(code_trimmed, &hash).unwrap_or(false) {
                matched_id = Some(id);
                break;
            }
        }
        if let Some(id) = matched_id {
            let consumed = state
                .totp
                .mark_recovery_code_used(id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            consumed
        } else {
            false
        }
    };

    if !verified {
        return Err(ApiError::Unauthorized("TOTP_CODE_INVALID"));
    }

    let auth = finish_login(&state, user_id, row.email, row.username).await?;
    Ok(Json(auth))
}

#[derive(Deserialize)]
pub struct TotpRequestDisableEmailRequest {
    pub pending_token: String,
}

/// `POST /api/auth/totp/request-disable-email`
/// 認証アプリ・リカバリーコードを両方失ったユーザー向け。パスワード検証は`pending_token`
/// の所有で証明済みのため、登録メールアドレス宛に強制解除ワンタイムリンクを送る。
pub async fn totp_request_disable_email(
    State(state): State<AppState>,
    Json(req): Json<TotpRequestDisableEmailRequest>,
) -> Result<Json<()>, ApiError> {
    let user_id = state
        .local_auth
        .verify_pending_totp_token(&req.pending_token)
        .map_err(|_| ApiError::Unauthorized("INVALID_PENDING_TOKEN"))?;

    let email = sqlx::query_scalar!("SELECT email FROM users WHERE id = $1", user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::BadRequest("USER_NOT_FOUND".to_owned()))?;

    let id = generate_snowflake_id(chrono::Utc::now());
    let token = state
        .totp
        .create_disable_request(id, user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::Internal("[totp-request-disable] token 発行失敗".to_owned()))?;

    let disable_url = format!(
        "https://{}/totp-disable?token={}",
        state.local_domain, token
    );

    let smtp_settings = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    send_totp_disable_email(&smtp_settings, &email, &disable_url)
        .await
        .map_err(|e| {
            tracing::error!("[totp-request-disable] メール送信失敗: {}", e);
            match e {
                crate::mailer::MailError::Config(_) => {
                    ApiError::ServiceUnavailable("SMTP_NOT_CONFIGURED")
                }
                _ => ApiError::Internal(format!("メール送信失敗: {}", e)),
            }
        })?;

    Ok(Json(()))
}

#[derive(Deserialize)]
pub struct TotpConfirmDisableRequest {
    pub token: String,
}

/// `POST /api/auth/totp/confirm-disable`
/// メールのリンクを踏んだ際に呼ぶ。パスワードリセット確認と同様、ログイン状態は
/// 要求しない（トークンの所有が証明）。
pub async fn totp_confirm_disable(
    State(state): State<AppState>,
    Json(req): Json<TotpConfirmDisableRequest>,
) -> Result<Json<()>, ApiError> {
    let token: uuid::Uuid = req
        .token
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_TOKEN".to_owned()))?;

    let user_id = state
        .totp
        .consume_disable_request(token)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("INVALID_TOKEN".to_owned()))?;

    state
        .totp
        .delete(user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(()))
}
