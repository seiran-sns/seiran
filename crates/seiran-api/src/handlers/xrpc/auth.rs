//! XRPC の呼び出し元の認証。`createSession` の accessJwt（`Authorization: Bearer`）と、
//! OAuth のアクセストークン（`Authorization: DPoP` と `DPoP` 証明）の両方をここで受け、
//! 各ハンドラには検証済みの DID・権限だけを渡す（トークンの種類を見分けさせない）。

use axum::{
    extract::{FromRequestParts, OriginalUri},
    http::{request::Parts, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

use seiran_common::repository::atp_oauth;

use super::{extract_bearer, service_did};
use crate::atproto_oauth::{
    self, check_dpop_jti, dpop, external_url, nonce_source, single_header, JtiError, SCOPE_EMAIL,
    SCOPE_GENERIC,
};
use crate::error::ApiError;
use crate::AppState;

/// 認証に要る要求の部分（メソッド・パス・ヘッダー）。DPoP 証明の `htm`/`htu` と照合する。
pub struct XrpcRequest {
    pub method: Method,
    pub path: String,
    pub headers: HeaderMap,
}

#[axum::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for XrpcRequest {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let path = parts
            .extensions
            .get::<OriginalUri>()
            .map(|u| u.0.path().to_string())
            .unwrap_or_else(|| parts.uri.path().to_string());
        Ok(Self {
            method: parts.method.clone(),
            path,
            headers: parts.headers.clone(),
        })
    }
}

/// エンドポイントが求める権限。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScopeNeed {
    /// 誰かが分かればよい（`getSession`）。OAuth の `atproto` だけでも通す。
    Identity,
    /// レコードの読み書き等、アプリパスワードと同じ範囲。OAuth なら `transition:generic` が要る。
    Generic,
}

/// 検証済みの呼び出し元。
pub struct AtpCaller {
    pub did: String,
    /// メインパスワードでログインしたセッションだけ真（PLC 操作・アカウント無効化に要る）。
    pub privileged: bool,
    /// メールアドレスを読ませてよいか（OAuth の `transition:email`）。
    pub email_scope: bool,
}

/// OAuth のアクセストークンでの失敗。`WWW-Authenticate` に理由を載せる（RFC 9449）。
fn dpop_error(
    state: &AppState,
    status: StatusCode,
    error: &'static str,
    message: &str,
) -> Response {
    let mut res = (
        status,
        Json(serde_json::json!({"error": error, "message": message})),
    )
        .into_response();
    let www = format!("DPoP error=\"{error}\", error_description=\"{message}\"");
    if let Ok(v) = HeaderValue::from_str(&www) {
        res.headers_mut().insert("www-authenticate", v);
    }
    atproto_oauth::attach_nonce(state, &mut res);
    res
}

pub async fn authenticate(
    state: &AppState,
    req: &XrpcRequest,
    need: ScopeNeed,
) -> Result<AtpCaller, Response> {
    let authorization = req
        .headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if let Some(token) = authorization.strip_prefix("DPoP ") {
        return authenticate_oauth(state, req, token, need).await;
    }
    let Some(token) = extract_bearer(&req.headers) else {
        return Err(ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response());
    };
    let verified = state
        .local_auth
        .verify_atp_access_token(token, &service_did(state))
        .map_err(|_| ApiError::Unauthorized("トークンが無効です").into_response())?;
    Ok(AtpCaller {
        did: verified.did,
        privileged: verified.privileged,
        email_scope: false,
    })
}

async fn authenticate_oauth(
    state: &AppState,
    req: &XrpcRequest,
    token: &str,
    need: ScopeNeed,
) -> Result<AtpCaller, Response> {
    let unauthorized =
        |error, message: &str| dpop_error(state, StatusCode::UNAUTHORIZED, error, message);
    let Some(proof) = single_header(&req.headers, "dpop") else {
        return Err(unauthorized("invalid_dpop_proof", "DPoP proof is required"));
    };
    let verified = dpop::verify(
        proof,
        &nonce_source(state),
        req.method.as_str(),
        &external_url(state, &req.path),
        Some(token),
    )
    .map_err(|e| match e {
        dpop::DpopError::UseNonce => unauthorized(
            "use_dpop_nonce",
            "Resource server requires nonce in DPoP proof",
        ),
        dpop::DpopError::Invalid(m) => unauthorized("invalid_dpop_proof", m),
    })?;
    check_dpop_jti(state, &verified)
        .await
        .map_err(|e| match e {
            JtiError::Reused => unauthorized("invalid_dpop_proof", "DPoP proof was already used"),
            JtiError::Unavailable(m) => ApiError::Internal(m).into_response(),
        })?;

    let access = atproto_oauth::token::verify_access_token(
        &state.secrets.jwt_secret_bytes(),
        &atproto_oauth::issuer(state),
        token,
    )
    .ok_or_else(|| unauthorized("invalid_token", "Invalid access token"))?;
    if access.jkt != verified.jkt {
        return Err(unauthorized(
            "invalid_token",
            "DPoP key does not match the access token",
        ));
    }
    // 設定画面での取り消しやパスワード変更をすぐ効かせるため、毎回セッションを確かめる。
    let session = atp_oauth::find_active_session(&state.db, access.session_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[atproto-oauth] {}", e)).into_response())?
        .ok_or_else(|| unauthorized("invalid_token", "Session is no longer valid"))?;
    let scopes: Vec<&str> = session.scope.split_whitespace().collect();
    if need == ScopeNeed::Generic && !scopes.contains(&SCOPE_GENERIC) {
        return Err(dpop_error(
            state,
            StatusCode::FORBIDDEN,
            "InsufficientScope",
            "transition:generic scope is required",
        ));
    }
    Ok(AtpCaller {
        did: access.did,
        privileged: false,
        email_scope: scopes.contains(&SCOPE_EMAIL),
    })
}

/// DPoP 証明付きの XRPC 要求への応答に、次の nonce を付ける（`routes` の層）。
pub async fn attach_dpop_nonce(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let wants_nonce = req.uri().path().starts_with("/xrpc/") && req.headers().contains_key("dpop");
    let mut res = next.run(req).await;
    if wants_nonce && !res.headers().contains_key("dpop-nonce") {
        atproto_oauth::attach_nonce(&state, &mut res);
    }
    res
}
