//! ATP アプリパスワードの発行・一覧・無効化。
//!
//! 設定画面（`/api/account/app-passwords`）と `com.atproto.server.*AppPassword`
//! （bsky.app 等の ATP クライアント）の両方から使う。発行・検証・失効はここに集め、
//! 呼び出し側は認証と応答の整形だけを持つ。

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use seiran_common::repository::Actor;
use seiran_common::{generate_snowflake_id, LocalAuthProvider};

use super::xrpc::{extract_bearer, require_privileged_session, service_did};
use crate::error::ApiError;
use crate::middleware::extract_auth;
use crate::AppState;

/// 名前の上限（設定画面のアプリトークンと同じ）。
const MAX_NAME_CHARS: usize = 100;

pub struct CreatedAppPassword {
    pub id: i64,
    pub name: String,
    pub password: String,
    pub created_at: DateTime<Utc>,
}

/// アプリパスワードを管理できるのは、seiran の通常ログイン（設定画面）か、メインパスワードで
/// 始めた ATP セッション（bsky.app の設定画面）。アプリパスワードのセッションに許すと、
/// 渡した相手が自分で新しいアプリパスワードを作って無効化をすり抜けられる（公式 PDS と同じ制限）。
pub async fn authenticate_manager(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Actor, ApiError> {
    if let Some(token) = extract_bearer(headers) {
        if let Ok(verified) = state
            .local_auth
            .verify_atp_access_token(token, &service_did(state))
        {
            require_privileged_session(&verified)?;
            let actor = state
                .actors
                .find_by_did(&verified.did)
                .await
                .map_err(|e| ApiError::Internal(format!("[app-passwords] DB エラー: {}", e)))?
                .filter(|a| a.user_id.is_some())
                .ok_or(ApiError::Unauthorized("アクターが見つかりません"))?;
            let user_id = actor.user_id.expect("filter済み");
            if state
                .users
                .is_suspended_by_user_id(user_id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?
            {
                return Err(ApiError::Forbidden("ACCOUNT_SUSPENDED"));
            }
            return Ok(actor);
        }
    }

    let auth_user = extract_auth(
        headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
    state
        .actors
        .find_local_by_user_id(auth_user.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[app-passwords] DB エラー: {}", e)))?
        .ok_or(ApiError::NotFound("ACTOR_NOT_FOUND"))
}

/// 発行する。平文のパスワードはこの戻り値でしか得られない（DB には argon2 ハッシュだけ残す）。
pub async fn create(
    state: &AppState,
    actor: &Actor,
    name: &str,
) -> Result<CreatedAppPassword, ApiError> {
    if actor.at_did.is_none() {
        return Err(ApiError::BadRequest("ATP_ACCOUNT_REQUIRED".to_string()));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "APP_PASSWORD_NAME_REQUIRED".to_string(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError::BadRequest(
            "APP_PASSWORD_NAME_TOO_LONG".to_string(),
        ));
    }

    let password = LocalAuthProvider::generate_app_password();
    let hash = LocalAuthProvider::hash_password(&password)
        .map_err(|e| ApiError::Internal(format!("[app-passwords] ハッシュ化失敗: {}", e)))?;
    let now = Utc::now();
    let id = generate_snowflake_id(now);
    let inserted = state
        .atp_sessions
        .insert_app_password(id, actor.id, name, &hash)
        .await
        .map_err(|e| ApiError::Internal(format!("[app-passwords] DB エラー: {}", e)))?;
    if !inserted {
        return Err(ApiError::Conflict("APP_PASSWORD_NAME_TAKEN"));
    }

    Ok(CreatedAppPassword {
        id,
        name: name.to_string(),
        password,
        created_at: now,
    })
}

pub async fn list(
    state: &AppState,
    actor: &Actor,
) -> Result<Vec<seiran_common::repository::AppPasswordRow>, ApiError> {
    state
        .atp_sessions
        .list_app_passwords(actor.id)
        .await
        .map_err(|e| ApiError::Internal(format!("[app-passwords] DB エラー: {}", e)))
}

/// 名前で無効化する（`revokeAppPassword` の引数が名前のため）。
pub async fn revoke_by_name(state: &AppState, actor: &Actor, name: &str) -> Result<(), ApiError> {
    let revoked = state
        .atp_sessions
        .revoke_app_password(actor.id, name)
        .await
        .map_err(|e| ApiError::Internal(format!("[app-passwords] DB エラー: {}", e)))?;
    if revoked {
        Ok(())
    } else {
        Err(ApiError::NotFound("APP_PASSWORD_NOT_FOUND"))
    }
}

pub async fn revoke_by_id(state: &AppState, actor: &Actor, id: i64) -> Result<(), ApiError> {
    let revoked = state
        .atp_sessions
        .revoke_app_password_by_id(actor.id, id)
        .await
        .map_err(|e| ApiError::Internal(format!("[app-passwords] DB エラー: {}", e)))?;
    if revoked {
        Ok(())
    } else {
        Err(ApiError::NotFound("APP_PASSWORD_NOT_FOUND"))
    }
}

// ─── 設定画面向け（`/api/account/app-passwords`） ─────────────────────────────

#[derive(Serialize)]
pub struct AppPasswordResponse {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct CreateAppPasswordResponse {
    pub id: String,
    pub name: String,
    pub password: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct CreateAppPasswordRequest {
    pub name: String,
}

/// `GET /api/account/app-passwords`
pub async fn list_handler(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<AppPasswordResponse>>, ApiError> {
    let actor = authenticate_manager(&state, &headers).await?;
    let rows = list(&state, &actor).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| AppPasswordResponse {
                id: r.id.to_string(),
                name: r.name,
                created_at: r.created_at,
            })
            .collect(),
    ))
}

/// `POST /api/account/app-passwords`
pub async fn create_handler(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateAppPasswordRequest>,
) -> Result<Json<CreateAppPasswordResponse>, ApiError> {
    let actor = authenticate_manager(&state, &headers).await?;
    let created = create(&state, &actor, &req.name).await?;
    Ok(Json(CreateAppPasswordResponse {
        id: created.id.to_string(),
        name: created.name,
        password: created.password,
        created_at: created.created_at,
    }))
}

/// `DELETE /api/account/app-passwords/:id`
pub async fn revoke_handler(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<()>, ApiError> {
    let actor = authenticate_manager(&state, &headers).await?;
    revoke_by_id(&state, &actor, id).await?;
    Ok(Json(()))
}
