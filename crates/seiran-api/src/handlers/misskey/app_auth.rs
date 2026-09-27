//! Misskey 旧来の app 認証フロー（app/create → auth/session/generate → auth/session/userkey）。
//! SocialHub Web 等、MiAuth 非対応の古いクライアントが使う。
//!
//! `oauth_apps` の仕組みは Mastodon 互換 OAuth（`handlers::mastodon::oauth`）と共用する。
//! Misskey の「secret」は client_secret と同じ役割だが、client_id を経由せず secret 単体で
//! アプリを特定する（session/generate・session/userkey はどちらも appSecret だけを送る）。
//! 承認前後のセッション（token → app_id → 承認後の user_id）は `misskey_auth_sessions` に
//! 持ち、承認確認画面は MiAuth/Mastodon OAuth と同じく SPA 側（`/misskey-connect/:token`）で
//! 行う。

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect},
    Json,
};
use serde::{Deserialize, Serialize};

use seiran_common::repository::oauth::{
    self, random_token, sha256_hex, NewMisskeyAuthSession, NewOAuthApp,
};

use crate::error::ApiError;
use crate::handlers::miauth::{build_check_response_user, is_valid_callback, CheckResponseUser};
use crate::middleware::AuthedUser;
use crate::AppState;

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

const SESSION_TTL_MINUTES: i64 = 30;

#[derive(Deserialize)]
pub struct AppCreateParams {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub permission: Vec<String>,
    #[serde(default)]
    #[serde(rename = "callbackUrl")]
    pub callback_url: Option<String>,
}

#[derive(Serialize)]
pub struct AppCreateResponse {
    pub id: String,
    pub name: String,
    #[serde(rename = "callbackUrl")]
    pub callback_url: Option<String>,
    pub permission: Vec<String>,
    pub secret: String,
}

/// `POST /api/app/create`（認証不要）
pub async fn app_create(
    State(state): State<AppState>,
    Json(params): Json<AppCreateParams>,
) -> Result<Json<AppCreateResponse>, ApiError> {
    let name = params.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".to_owned()));
    }
    if let Some(cb) = &params.callback_url {
        if !is_valid_callback(cb) {
            return Err(ApiError::BadRequest("invalid callbackUrl".to_owned()));
        }
    }

    let id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    // client_id は内部識別用のみ。Misskey クライアントには一切返さない
    // （Misskey の app/create レスポンスに client_id という概念自体が無いため）。
    let client_id = random_token();
    let secret = random_token();
    let redirect_uris: Vec<String> = params.callback_url.iter().cloned().collect();
    let description = (!params.description.trim().is_empty()).then_some(params.description);

    oauth::insert_app(
        &state.db,
        &NewOAuthApp {
            id,
            client_id: &client_id,
            client_secret_hash: &sha256_hex(&secret),
            name,
            redirect_uris: &redirect_uris,
            scopes: &params.permission.join(" "),
            website: None,
            description: description.as_deref(),
        },
    )
    .await
    .map_err(internal)?;

    Ok(Json(AppCreateResponse {
        id: id.to_string(),
        name: name.to_owned(),
        callback_url: params.callback_url,
        permission: params.permission,
        secret,
    }))
}

#[derive(Deserialize)]
pub struct SessionGenerateParams {
    #[serde(rename = "appSecret")]
    pub app_secret: String,
}

#[derive(Serialize)]
pub struct SessionGenerateResponse {
    pub token: String,
    pub url: String,
}

/// `POST /api/auth/session/generate`（認証不要）
pub async fn session_generate(
    State(state): State<AppState>,
    Json(params): Json<SessionGenerateParams>,
) -> Result<Json<SessionGenerateResponse>, ApiError> {
    let app = oauth::find_app_by_client_secret_hash(&state.db, &sha256_hex(&params.app_secret))
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::BadRequest("no such app".to_owned()))?;

    let token = random_token();
    oauth::insert_misskey_auth_session(
        &state.db,
        &NewMisskeyAuthSession {
            token_hash: &sha256_hex(&token),
            app_id: app.id,
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(SESSION_TTL_MINUTES),
        },
    )
    .await
    .map_err(internal)?;

    let url = format!("https://{}/auth/{}", state.local_domain, token);
    Ok(Json(SessionGenerateResponse { token, url }))
}

/// `GET /auth/:token`。third-party クライアントが最初に開く URL。MiAuth の `miauth_page` と
/// 同じく、検証は行わず SPA（`/misskey-connect/:token`）へ薄くリダイレクトするだけ。
/// アプリ名の取得・改ざん防止は SPA が `GET /api/auth-sessions/:token` で行う。
pub async fn auth_page(Path(token): Path<String>) -> impl IntoResponse {
    Redirect::to(&format!("/misskey-connect/{token}"))
}

#[derive(Serialize)]
pub struct SessionInfoResponse {
    pub name: String,
    pub description: Option<String>,
    pub permission: Vec<String>,
    #[serde(rename = "callbackUrl")]
    pub callback_url: Option<String>,
}

/// `GET /api/auth-sessions/:token`（認証不要）。SPA の承認確認画面に出すアプリ名。
/// Mastodon の `app_info` と同じ理由（URL のクエリではなくサーバー側の登録内容から引く、
/// 他のアプリへのなりすまし防止）でこのエンドポイントを分けている。
pub async fn session_info(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<SessionInfoResponse>, ApiError> {
    let session = oauth::find_misskey_auth_session(&state.db, &sha256_hex(&token))
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("AUTH_SESSION_NOT_FOUND"))?;
    Ok(Json(SessionInfoResponse {
        name: session.name,
        description: session.description,
        permission: session
            .scopes
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        callback_url: session.redirect_uris.into_iter().next(),
    }))
}

/// `POST /api/auth-sessions/:token/authorize`（Bearer 認証必須）。SPA の承認確認画面から呼ぶ。
pub async fn session_authorize(
    user: AuthedUser,
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let approved = oauth::approve_misskey_auth_session(
        &state.db,
        &sha256_hex(&token),
        user.user_id,
    )
    .await
    .map_err(internal)?;
    if !approved {
        return Err(ApiError::NotFound("AUTH_SESSION_NOT_FOUND"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct SessionUserkeyParams {
    #[serde(rename = "appSecret")]
    pub app_secret: String,
    pub token: String,
}

#[derive(Serialize)]
pub struct SessionUserkeyResponse {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    pub user: CheckResponseUser,
}

/// `POST /api/auth/session/userkey`（認証不要）。承認済みなら一度きり発行し、
/// 未承認・期限切れ・二回目の呼び出しは失敗する。
pub async fn session_userkey(
    State(state): State<AppState>,
    Json(params): Json<SessionUserkeyParams>,
) -> Result<Json<SessionUserkeyResponse>, ApiError> {
    let app = oauth::find_app_by_client_secret_hash(&state.db, &sha256_hex(&params.app_secret))
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::BadRequest("no such app".to_owned()))?;

    let consumed = oauth::consume_misskey_auth_session(&state.db, &sha256_hex(&params.token), app.id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            ApiError::BadRequest("This session is not approved yet, or already used.".to_owned())
        })?;

    let actor = state
        .actors
        .find_local_by_user_id(consumed.user_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("USER_NOT_FOUND"))?;

    let (access_token, jti) = state
        .local_auth
        .generate_app_token(consumed.user_id, &consumed.email)
        .map_err(internal)?;
    // Misskey クライアント（misskey4j 等）はこのトークンを生のまま送らず、
    // `sha256(accessToken + appSecret)` を `i` として送ってくる（本家 Misskey の
    // `AuthenticateService` も同じ形式を `hash` 列との照合で受け付ける）。この値を
    // 保存しておき、`extract_auth` が JWT として検証できなかった受信値をこれで引き当てる。
    let misskey_hash = sha256_hex(&format!("{access_token}{}", params.app_secret));
    oauth::insert_app_token(
        &state.db,
        jti,
        consumed.user_id,
        &app.name,
        app.id,
        Some(&misskey_hash),
    )
    .await
    .map_err(internal)?;

    let user = build_check_response_user(&state, actor.id, actor.username).await;
    Ok(Json(SessionUserkeyResponse { access_token, user }))
}
