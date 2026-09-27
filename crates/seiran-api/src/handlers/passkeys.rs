//! WebAuthnパスキー: 複数登録・削除・パスワードレスログイン（#65）。

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use seiran_common::repository::passkey;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use webauthn_rs::prelude::{
    DiscoverableAuthentication, DiscoverableKey, Passkey, PasskeyRegistration, PublicKeyCredential,
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
    let user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
    let rows = passkey::list_for_user(&state.db, user.user_id)
        .await
        .map_err(internal)?;
    Ok(Json(
        rows.into_iter()
            .map(|row| PasskeySummary {
                id: row.id,
                name: row.name,
                created_at: row.created_at,
                last_used_at: row.last_used_at,
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
    let user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::BadRequest("PASSKEY_NAME_INVALID".into()));
    }
    let username = local_username(&state, user.user_id).await?;
    let existing = load_passkeys(&state, user.user_id).await?;
    let exclude = existing
        .iter()
        .map(|(_, passkey)| passkey.cred_id().clone())
        .collect();
    // usernameless(discoverable credential)ログインのため、resident key必須・
    // プラットフォーム認証器限定で登録する(#65-2)。USBセキュリティキーでの登録は不可になる。
    let (public_key, reg_state) = state
        .webauthn
        .start_google_passkey_in_google_password_manager_only_registration(
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
    save_challenge(
        &state,
        token,
        Some(user.user_id),
        "registration",
        state_json,
    )
    .await?;
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
    let user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
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
    let created_at = passkey::insert(&state.db, id, user.user_id, name, credential)
        .await
        .map_err(internal)?;
    Ok(Json(PasskeySummary {
        id,
        name: name.to_owned(),
        created_at,
        last_used_at: None,
    }))
}

pub async fn delete(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(), ApiError> {
    let user = extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
    let deleted = passkey::delete(&state.db, id, user.user_id)
        .await
        .map_err(internal)?;
    if !deleted {
        return Err(ApiError::NotFound("PASSKEY_NOT_FOUND"));
    }
    Ok(())
}

pub async fn authentication_start(
    State(state): State<AppState>,
) -> Result<Json<ChallengeResponse<webauthn_rs::prelude::RequestChallengeResponse>>, ApiError> {
    let (public_key, auth_state) = state
        .webauthn
        .start_discoverable_authentication()
        .map_err(webauthn_error)?;
    let token = Uuid::new_v4();
    save_challenge(
        &state,
        token,
        None,
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
    let state_value = passkey::consume_challenge(&state.db, req.token, None, "authentication")
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::BadRequest("PASSKEY_CHALLENGE_INVALID".into()))?;
    let auth_state: DiscoverableAuthentication =
        serde_json::from_value(state_value).map_err(internal)?;

    let (user_unique_id, _) = state
        .webauthn
        .identify_discoverable_authentication(&req.credential)
        .map_err(webauthn_error)?;
    let user_id = user_unique_id.as_u128() as i64;

    let passkeys = load_passkeys(&state, user_id).await?;
    let discoverable_creds: Vec<DiscoverableKey> =
        passkeys.iter().map(|(_, passkey)| passkey.into()).collect();
    let result = state
        .webauthn
        .finish_discoverable_authentication(&req.credential, auth_state, &discoverable_creds)
        .map_err(webauthn_error)?;

    let credential_id = result.cred_id();
    let (id, mut passkey) = passkeys
        .into_iter()
        .find(|(_, passkey)| passkey.cred_id() == credential_id)
        .ok_or(ApiError::Unauthorized("PASSKEY_INVALID"))?;
    passkey.update_credential(&result);
    let credential = serde_json::to_value(passkey).map_err(internal)?;
    passkey::record_use(&state.db, id, user_id, credential)
        .await
        .map_err(internal)?;

    let login = state
        .users
        .find_login_by_username(&local_username(&state, user_id).await?)
        .await
        .map_err(internal)?
        .ok_or(ApiError::Unauthorized("PASSKEY_INVALID"))?;
    Ok(Json(
        finish_login(&state, user_id, login.email, login.username).await?,
    ))
}

async fn load_passkeys(state: &AppState, user_id: i64) -> Result<Vec<(Uuid, Passkey)>, ApiError> {
    passkey::credentials_for_user(&state.db, user_id)
        .await
        .map_err(internal)?
        .into_iter()
        .map(|(id, value)| Ok((id, serde_json::from_value(value).map_err(internal)?)))
        .collect()
}

async fn local_username(state: &AppState, user_id: i64) -> Result<String, ApiError> {
    state
        .actors
        .find_local_by_user_id(user_id)
        .await
        .map_err(internal)?
        .map(|a| a.username)
        .ok_or_else(|| ApiError::Internal("ローカルアクターが見つかりません".to_string()))
}

async fn save_challenge(
    state: &AppState,
    token: Uuid,
    user_id: Option<i64>,
    kind: &str,
    value: serde_json::Value,
) -> Result<(), ApiError> {
    passkey::save_challenge(&state.db, token, user_id, kind, value)
        .await
        .map_err(internal)
}

async fn consume_challenge(
    state: &AppState,
    token: Uuid,
    user_id: i64,
    kind: &str,
) -> Result<serde_json::Value, ApiError> {
    passkey::consume_challenge(&state.db, token, Some(user_id), kind)
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
