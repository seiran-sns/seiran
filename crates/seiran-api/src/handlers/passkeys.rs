//! WebAuthnパスキー: 複数登録・削除・パスワードレスログイン（#65）。

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential,
};

use crate::handlers::auth::{finish_login, AuthResponse};
use crate::{error::ApiError, middleware::extract_auth, AppState};

#[derive(Serialize)]
pub struct PasskeySummary {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<PasskeySummary>>, ApiError> {
    let user = extract_auth(&headers, &state.local_auth, state.app_tokens.as_ref()).await?;
    let rows = sqlx::query(
        "SELECT id, name, created_at, last_used_at
         FROM user_passkeys WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user.user_id)
    .fetch_all(&state.db)
    .await
    .map_err(internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|row| PasskeySummary {
                id: row.get("id"),
                name: row.get("name"),
                created_at: row.get("created_at"),
                last_used_at: row.get("last_used_at"),
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct RegistrationStartRequest {
    pub name: String,
}

#[derive(Serialize)]
pub struct ChallengeResponse<T> {
    pub token: Uuid,
    pub public_key: T,
}

pub async fn registration_start(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<RegistrationStartRequest>,
) -> Result<Json<ChallengeResponse<webauthn_rs::prelude::CreationChallengeResponse>>, ApiError> {
    let user = extract_auth(&headers, &state.local_auth, state.app_tokens.as_ref()).await?;
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::BadRequest("PASSKEY_NAME_INVALID".into()));
    }
    let username: String = sqlx::query_scalar(
        "SELECT username FROM actors WHERE user_id = $1 AND actor_type::text = 'local'",
    )
    .bind(user.user_id)
    .fetch_one(&state.db)
    .await
    .map_err(internal)?;
    let existing = load_passkeys(&state, user.user_id).await?;
    let exclude = existing
        .iter()
        .map(|(_, passkey)| passkey.cred_id().clone())
        .collect();
    let (public_key, reg_state) = state
        .webauthn
        .start_passkey_registration(
            Uuid::from_u128(user.user_id as u128),
            &username,
            &username,
            Some(exclude),
        )
        .map_err(webauthn_error)?;
    let token = Uuid::new_v4();
    let state_json = serde_json::json!({
        "name": name,
        "registration": reg_state,
    });
    save_challenge(&state, token, user.user_id, "registration", state_json).await?;
    Ok(Json(ChallengeResponse { token, public_key }))
}

#[derive(Deserialize)]
pub struct RegistrationFinishRequest {
    pub token: Uuid,
    pub credential: RegisterPublicKeyCredential,
}

pub async fn registration_finish(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<RegistrationFinishRequest>,
) -> Result<Json<PasskeySummary>, ApiError> {
    let user = extract_auth(&headers, &state.local_auth, state.app_tokens.as_ref()).await?;
    let value = consume_challenge(&state, req.token, user.user_id, "registration").await?;
    let name = value["name"]
        .as_str()
        .ok_or_else(|| ApiError::BadRequest("PASSKEY_CHALLENGE_INVALID".into()))?;
    let reg_state: PasskeyRegistration = serde_json::from_value(value["registration"].clone())
        .map_err(|_| ApiError::BadRequest("PASSKEY_CHALLENGE_INVALID".into()))?;
    let passkey = state
        .webauthn
        .finish_passkey_registration(&req.credential, &reg_state)
        .map_err(webauthn_error)?;
    let id = Uuid::new_v4();
    let credential = serde_json::to_value(passkey).map_err(internal)?;
    let row = sqlx::query(
        "INSERT INTO user_passkeys (id, user_id, name, credential)
         VALUES ($1, $2, $3, $4)
         RETURNING created_at",
    )
    .bind(id)
    .bind(user.user_id)
    .bind(name)
    .bind(credential)
    .fetch_one(&state.db)
    .await
    .map_err(internal)?;
    Ok(Json(PasskeySummary {
        id,
        name: name.to_owned(),
        created_at: row.get("created_at"),
        last_used_at: None,
    }))
}

pub async fn delete(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(), ApiError> {
    let user = extract_auth(&headers, &state.local_auth, state.app_tokens.as_ref()).await?;
    let result = sqlx::query("DELETE FROM user_passkeys WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.user_id)
        .execute(&state.db)
        .await
        .map_err(internal)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("PASSKEY_NOT_FOUND"));
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct AuthenticationStartRequest {
    pub identifier: String,
}

pub async fn authentication_start(
    State(state): State<AppState>,
    Json(req): Json<AuthenticationStartRequest>,
) -> Result<Json<ChallengeResponse<webauthn_rs::prelude::RequestChallengeResponse>>, ApiError> {
    let row = if req.identifier.contains('@') {
        state.users.find_login_by_email(&req.identifier).await
    } else {
        state.users.find_login_by_username(&req.identifier).await
    }
    .map_err(internal)?
    .ok_or(ApiError::Unauthorized("PASSKEY_NOT_AVAILABLE"))?;
    let passkeys = load_passkeys(&state, row.id).await?;
    if passkeys.is_empty() {
        return Err(ApiError::Unauthorized("PASSKEY_NOT_AVAILABLE"));
    }
    let credentials: Vec<Passkey> = passkeys.into_iter().map(|(_, passkey)| passkey).collect();
    let (public_key, auth_state) = state
        .webauthn
        .start_passkey_authentication(&credentials)
        .map_err(webauthn_error)?;
    let token = Uuid::new_v4();
    save_challenge(
        &state,
        token,
        row.id,
        "authentication",
        serde_json::to_value(auth_state).map_err(internal)?,
    )
    .await?;
    Ok(Json(ChallengeResponse { token, public_key }))
}

#[derive(Deserialize)]
pub struct AuthenticationFinishRequest {
    pub token: Uuid,
    pub credential: PublicKeyCredential,
}

pub async fn authentication_finish(
    State(state): State<AppState>,
    Json(req): Json<AuthenticationFinishRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let row = sqlx::query(
        "DELETE FROM passkey_challenges
         WHERE token = $1 AND kind = 'authentication' AND expires_at > now()
         RETURNING user_id, state",
    )
    .bind(req.token)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or_else(|| ApiError::BadRequest("PASSKEY_CHALLENGE_INVALID".into()))?;
    let user_id: i64 = row.get("user_id");
    let auth_state: PasskeyAuthentication =
        serde_json::from_value(row.get("state")).map_err(internal)?;
    let result = state
        .webauthn
        .finish_passkey_authentication(&req.credential, &auth_state)
        .map_err(webauthn_error)?;

    let credential_id = result.cred_id();
    let passkeys = load_passkeys(&state, user_id).await?;
    let (id, mut passkey) = passkeys
        .into_iter()
        .find(|(_, passkey)| passkey.cred_id() == credential_id)
        .ok_or(ApiError::Unauthorized("PASSKEY_INVALID"))?;
    passkey.update_credential(&result);
    sqlx::query(
        "UPDATE user_passkeys SET credential = $1, last_used_at = now()
         WHERE id = $2 AND user_id = $3",
    )
    .bind(serde_json::to_value(passkey).map_err(internal)?)
    .bind(id)
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(internal)?;

    let login = state
        .users
        .find_login_by_username(
            &sqlx::query_scalar::<_, String>(
                "SELECT username FROM actors WHERE user_id = $1 AND actor_type::text = 'local'",
            )
            .bind(user_id)
            .fetch_one(&state.db)
            .await
            .map_err(internal)?,
        )
        .await
        .map_err(internal)?
        .ok_or(ApiError::Unauthorized("PASSKEY_INVALID"))?;
    Ok(Json(
        finish_login(&state, user_id, login.email, login.username).await?,
    ))
}

async fn load_passkeys(state: &AppState, user_id: i64) -> Result<Vec<(Uuid, Passkey)>, ApiError> {
    let rows = sqlx::query("SELECT id, credential FROM user_passkeys WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(&state.db)
        .await
        .map_err(internal)?;
    rows.into_iter()
        .map(|row| {
            let value: serde_json::Value = row.get("credential");
            Ok((
                row.get("id"),
                serde_json::from_value(value).map_err(internal)?,
            ))
        })
        .collect()
}

async fn save_challenge(
    state: &AppState,
    token: Uuid,
    user_id: i64,
    kind: &str,
    value: serde_json::Value,
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM passkey_challenges WHERE expires_at <= now()")
        .execute(&state.db)
        .await
        .map_err(internal)?;
    sqlx::query(
        "INSERT INTO passkey_challenges (token, user_id, kind, state) VALUES ($1, $2, $3, $4)",
    )
    .bind(token)
    .bind(user_id)
    .bind(kind)
    .bind(value)
    .execute(&state.db)
    .await
    .map_err(internal)?;
    Ok(())
}

async fn consume_challenge(
    state: &AppState,
    token: Uuid,
    user_id: i64,
    kind: &str,
) -> Result<serde_json::Value, ApiError> {
    sqlx::query_scalar(
        "DELETE FROM passkey_challenges
         WHERE token = $1 AND user_id = $2 AND kind = $3 AND expires_at > now()
         RETURNING state",
    )
    .bind(token)
    .bind(user_id)
    .bind(kind)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or_else(|| ApiError::BadRequest("PASSKEY_CHALLENGE_INVALID".into()))
}

fn webauthn_error(error: impl std::fmt::Display) -> ApiError {
    tracing::warn!("[passkey] WebAuthn検証失敗: {}", error);
    ApiError::BadRequest("PASSKEY_INVALID".into())
}

fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}
