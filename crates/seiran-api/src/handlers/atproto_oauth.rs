//! AT Protocol OAuth のエンドポイント（メタデータ・PAR・認可・トークン・失効）と、
//! SPA の承認画面・設定画面が呼ぶ API。流れは `docs/protocols.md` 3節「OAuth」。

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use seiran_common::generate_snowflake_id;
use seiran_common::repository::atp_oauth::{
    self, NewOAuthRequest, NewOAuthSession, RefreshRotation,
};
use seiran_common::repository::Actor;

use crate::atproto_oauth::client::{self, Client};
use crate::atproto_oauth::token::{self, NewAccessToken};
use crate::atproto_oauth::{
    attach_nonce, external_url, grantable_scope, issuer, verify_endpoint_dpop, OAuthError,
    CODE_TTL_SECS, CONFIDENTIAL_SESSION_TTL_DAYS, PATH_AUTHORIZE, PATH_PAR, PATH_REVOKE,
    PATH_TOKEN, PUBLIC_SESSION_TTL_DAYS, REQUEST_TTL_SECS, REQUEST_URI_PREFIX, SUPPORTED_SCOPES,
};
use crate::error::ApiError;
use crate::middleware::extract_auth;
use crate::AppState;

// ─── メタデータ ──────────────────────────────────────────────────────────────

/// `GET /.well-known/oauth-protected-resource`
pub async fn protected_resource_metadata(State(state): State<AppState>) -> Response {
    let issuer = issuer(&state);
    Json(serde_json::json!({
        "resource": issuer,
        "authorization_servers": [issuer],
        "scopes_supported": SUPPORTED_SCOPES,
        "bearer_methods_supported": ["header"],
    }))
    .into_response()
}

/// `GET /.well-known/oauth-authorization-server`
pub async fn authorization_server_metadata(State(state): State<AppState>) -> Response {
    let issuer = issuer(&state);
    Json(serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": external_url(&state, PATH_AUTHORIZE),
        "token_endpoint": external_url(&state, PATH_TOKEN),
        "pushed_authorization_request_endpoint": external_url(&state, PATH_PAR),
        "revocation_endpoint": external_url(&state, PATH_REVOKE),
        "require_pushed_authorization_requests": true,
        "response_types_supported": ["code"],
        "response_modes_supported": ["query", "fragment"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "private_key_jwt"],
        "token_endpoint_auth_signing_alg_values_supported": ["ES256"],
        "dpop_signing_alg_values_supported": ["ES256"],
        "scopes_supported": SUPPORTED_SCOPES,
        "subject_types_supported": ["public"],
        "authorization_response_iss_parameter_supported": true,
        "client_id_metadata_document_supported": true,
        "require_request_uri_registration": true,
        "protected_resources": [issuer],
    }))
    .into_response()
}

// ─── 要求の本文 ──────────────────────────────────────────────────────────────

/// フォーム（`x-www-form-urlencoded`）か JSON の本文を、文字列の値の表にする。同じ名前が
/// 2回出てきたら拒否する（どちらを使うかで検証をすり抜けられないように）。
fn parse_params(headers: &HeaderMap, body: &Bytes) -> Result<HashMap<String, String>, OAuthError> {
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let mut map = HashMap::new();
    if content_type.starts_with("application/json") {
        let value: serde_json::Value = serde_json::from_slice(body)
            .map_err(|_| OAuthError::invalid_request("invalid JSON body"))?;
        let obj = value
            .as_object()
            .ok_or_else(|| OAuthError::invalid_request("body must be an object"))?;
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                map.insert(k.clone(), s.to_string());
            }
        }
        return Ok(map);
    }
    for (k, v) in url::form_urlencoded::parse(body) {
        if map.insert(k.into_owned(), v.into_owned()).is_some() {
            return Err(OAuthError::invalid_request("duplicate parameter"));
        }
    }
    Ok(map)
}

fn param<'a>(params: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    params
        .get(name)
        .map(String::as_str)
        .filter(|s| !s.is_empty())
}

/// トークン系エンドポイントの応答には、成功・失敗を問わず次の DPoP nonce を付ける。
fn with_nonce(state: &AppState, result: Result<Response, OAuthError>) -> Response {
    let mut res = result.unwrap_or_else(IntoResponse::into_response);
    attach_nonce(state, &mut res);
    res
}

/// クライアントを解決し、機密クライアントなら `client_assertion` を検証して鍵の `kid` を返す。
async fn authenticate_client(
    state: &AppState,
    params: &HashMap<String, String>,
) -> Result<(Client, Option<String>), OAuthError> {
    let client_id = param(params, "client_id")
        .ok_or_else(|| OAuthError::invalid_request("client_id is required"))?;
    let client = client::resolve(client_id)
        .await
        .map_err(|e| OAuthError::invalid_client(e.0))?;
    if !client.is_confidential() {
        return Ok((client, None));
    }
    let kid = client::verify_client_assertion(
        &client,
        param(params, "client_assertion_type"),
        param(params, "client_assertion"),
        &issuer(state),
        &state.replay_guard,
    )
    .await
    .map_err(|e| OAuthError::invalid_client(e.0))?;
    Ok((client, Some(kid)))
}

// ─── PAR ─────────────────────────────────────────────────────────────────────

/// `POST /oauth/atproto/par`
pub async fn par(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let result = par_inner(&state, &headers, &body).await;
    with_nonce(&state, result)
}

/// PAR で保存する要求のパラメータ（`atp_oauth_requests.params`）。
#[derive(Serialize, Deserialize)]
struct StoredParams {
    redirect_uri: String,
    state: String,
    scope: Vec<String>,
    code_challenge: String,
    response_mode: String,
    login_hint: Option<String>,
}

fn is_code_challenge(s: &str) -> bool {
    (43..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

async fn par_inner(
    state: &AppState,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Response, OAuthError> {
    let jkt = verify_endpoint_dpop(state, headers, PATH_PAR).await?;
    let params = parse_params(headers, body)?;
    let (client, kid) = authenticate_client(state, &params).await?;

    if params.contains_key("request") || params.contains_key("request_uri") {
        return Err(OAuthError::invalid_request(
            "request objects are not supported",
        ));
    }
    if param(&params, "response_type") != Some("code") {
        return Err(OAuthError::invalid_request("response_type must be code"));
    }
    if param(&params, "code_challenge_method") != Some("S256") {
        return Err(OAuthError::invalid_request(
            "code_challenge_method must be S256",
        ));
    }
    let code_challenge = param(&params, "code_challenge")
        .filter(|c| is_code_challenge(c))
        .ok_or_else(|| OAuthError::invalid_request("invalid code_challenge"))?;
    let oauth_state =
        param(&params, "state").ok_or_else(|| OAuthError::invalid_request("state is required"))?;
    let redirect_uri = param(&params, "redirect_uri")
        .ok_or_else(|| OAuthError::invalid_request("redirect_uri is required"))?;
    if !client.allows_redirect_uri(redirect_uri) {
        return Err(OAuthError::invalid_request(
            "redirect_uri is not registered",
        ));
    }
    let response_mode = param(&params, "response_mode").unwrap_or("query");
    if !matches!(response_mode, "query" | "fragment") {
        return Err(OAuthError::invalid_request("unsupported response_mode"));
    }
    let requested = client::split_scope(param(&params, "scope").unwrap_or_default());
    if let Some(unknown) = requested.iter().find(|s| !client.scopes.contains(s)) {
        return Err(OAuthError {
            status: StatusCode::BAD_REQUEST,
            error: "invalid_scope",
            description: format!("scope {unknown} is not registered by the client"),
        });
    }
    if grantable_scope(&requested, &client.scopes).is_none() {
        return Err(OAuthError {
            status: StatusCode::BAD_REQUEST,
            error: "invalid_scope",
            description: "scope must include atproto".to_string(),
        });
    }

    let stored = StoredParams {
        redirect_uri: redirect_uri.to_string(),
        state: oauth_state.to_string(),
        scope: requested,
        code_challenge: code_challenge.to_string(),
        response_mode: response_mode.to_string(),
        login_hint: param(&params, "login_hint").map(str::to_string),
    };
    let id = token::random_token();
    let params_json = serde_json::to_value(&stored).map_err(OAuthError::server_error)?;
    atp_oauth::insert_request(
        &state.db,
        &NewOAuthRequest {
            id: &id,
            client_id: &client.client_id,
            client_name: client.client_name.as_deref(),
            params: &params_json,
            dpop_jkt: &jkt,
            client_kid: kid.as_deref(),
            expires_at: Utc::now() + Duration::seconds(REQUEST_TTL_SECS),
        },
    )
    .await
    .map_err(OAuthError::server_error)?;

    let mut res = (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "request_uri": format!("{REQUEST_URI_PREFIX}{id}"),
            "expires_in": REQUEST_TTL_SECS,
        })),
    )
        .into_response();
    res.headers_mut()
        .insert("cache-control", "no-store".parse().expect("static"));
    Ok(res)
}

// ─── 認可（ブラウザ） ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AuthorizeQuery {
    client_id: Option<String>,
    request_uri: Option<String>,
}

fn request_id(request_uri: &str) -> Option<&str> {
    request_uri
        .strip_prefix(REQUEST_URI_PREFIX)
        .filter(|id| !id.is_empty())
}

/// `GET /oauth/atproto/authorize` — 要求を確かめて SPA の承認画面へ送る。ログインしていなければ
/// SPA が通常のログインを経てから承認画面に戻す。
pub async fn authorize(State(state): State<AppState>, Query(q): Query<AuthorizeQuery>) -> Response {
    let (Some(client_id), Some(request_uri)) = (q.client_id.as_deref(), q.request_uri.as_deref())
    else {
        return (
            StatusCode::BAD_REQUEST,
            "client_id and request_uri are required",
        )
            .into_response();
    };
    let Some(id) = request_id(request_uri) else {
        return (StatusCode::BAD_REQUEST, "invalid request_uri").into_response();
    };
    match atp_oauth::find_pending_request(&state.db, id).await {
        Ok(Some(req)) if req.client_id == client_id => {}
        Ok(_) => {
            return (StatusCode::BAD_REQUEST, "unknown or expired request_uri").into_response()
        }
        Err(e) => return ApiError::Internal(format!("[atproto-oauth] {}", e)).into_response(),
    }
    let target = format!(
        "/oauth-atproto?request_uri={}",
        urlencoding::encode(request_uri)
    );
    Redirect::to(&target).into_response()
}

/// 承認画面の操作者（ログイン中の seiran ユーザーのアクター）。
async fn authed_actor(state: &AppState, headers: &HeaderMap) -> Result<Actor, ApiError> {
    let user = extract_auth(
        headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
    state
        .actors
        .find_local_by_user_id(user.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("[atproto-oauth] {}", e)))?
        .ok_or(ApiError::NotFound("ACTOR_NOT_FOUND"))
}

async fn load_pending(
    state: &AppState,
    request_uri: &str,
) -> Result<(atp_oauth::OAuthRequestRow, StoredParams), ApiError> {
    let id = request_id(request_uri).ok_or(ApiError::NotFound("OAUTH_REQUEST_NOT_FOUND"))?;
    let req = atp_oauth::find_pending_request(&state.db, id)
        .await
        .map_err(|e| ApiError::Internal(format!("[atproto-oauth] {}", e)))?
        .ok_or(ApiError::NotFound("OAUTH_REQUEST_NOT_FOUND"))?;
    let params: StoredParams = serde_json::from_value(req.params.clone()).map_err(|e| {
        ApiError::Internal(format!("[atproto-oauth] 保存した要求を読めません: {}", e))
    })?;
    Ok((req, params))
}

fn handle_of(state: &AppState, actor: &Actor) -> String {
    format!("{}.{}", actor.username, state.local_domain)
}

#[derive(Deserialize)]
pub struct RequestUriQuery {
    request_uri: String,
}

#[derive(Serialize)]
pub struct AuthorizationRequestInfo {
    client_id: String,
    client_host: String,
    client_name: Option<String>,
    client_uri: Option<String>,
    logo_uri: Option<String>,
    scopes: Vec<String>,
    handle: String,
    did: String,
    login_hint: Option<String>,
    login_hint_matches: bool,
}

/// `GET /api/atproto-oauth/request?request_uri=` — 承認画面に出す内容。
pub async fn request_info(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<RequestUriQuery>,
) -> Result<Json<AuthorizationRequestInfo>, ApiError> {
    let actor = authed_actor(&state, &headers).await?;
    let did = actor
        .at_did
        .clone()
        .ok_or_else(|| ApiError::BadRequest("ATP_ACCOUNT_REQUIRED".to_string()))?;
    let (req, params) = load_pending(&state, &q.request_uri).await?;
    let client = client::resolve(&req.client_id)
        .await
        .map_err(|_| ApiError::BadGateway("OAUTH_CLIENT_UNAVAILABLE".to_string()))?;
    let handle = handle_of(&state, &actor);
    let login_hint_matches = params.login_hint.as_deref().is_none_or(|hint| {
        let hint = hint.trim_start_matches('@');
        hint == did || hint.eq_ignore_ascii_case(&handle)
    });
    Ok(Json(AuthorizationRequestInfo {
        client_host: client.host(),
        client_id: client.client_id.clone(),
        client_name: client.client_name.clone(),
        client_uri: client.client_uri.clone(),
        logo_uri: client.logo_uri.clone(),
        scopes: grantable_scope(&params.scope, &client.scopes).unwrap_or_default(),
        handle,
        did,
        login_hint: params.login_hint,
        login_hint_matches,
    }))
}

#[derive(Deserialize)]
pub struct DecisionRequest {
    request_uri: String,
}

#[derive(Serialize)]
pub struct DecisionResponse {
    redirect_url: String,
}

/// クライアントの `redirect_uri` に結果を付けた URL。`iss` を必ず付ける（クライアントが
/// 別の認可サーバーからの応答と取り違えないように）。
fn redirect_with(state: &AppState, params: &StoredParams, pairs: &[(&str, &str)]) -> String {
    let iss = issuer(state);
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        ser.append_pair(k, v);
    }
    ser.append_pair("state", &params.state);
    ser.append_pair("iss", &iss);
    let encoded = ser.finish();
    let sep = if params.response_mode == "fragment" {
        '#'
    } else if params.redirect_uri.contains('?') {
        '&'
    } else {
        '?'
    };
    format!("{}{}{}", params.redirect_uri, sep, encoded)
}

/// `POST /api/atproto-oauth/approve` — 承認して認可コードを発行し、戻り先を返す。
pub async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DecisionRequest>,
) -> Result<Json<DecisionResponse>, ApiError> {
    let actor = authed_actor(&state, &headers).await?;
    if actor.at_did.is_none() {
        return Err(ApiError::BadRequest("ATP_ACCOUNT_REQUIRED".to_string()));
    }
    let (req, params) = load_pending(&state, &body.request_uri).await?;
    let client = client::resolve(&req.client_id)
        .await
        .map_err(|_| ApiError::BadGateway("OAUTH_CLIENT_UNAVAILABLE".to_string()))?;
    let granted = grantable_scope(&params.scope, &client.scopes)
        .ok_or_else(|| ApiError::BadRequest("OAUTH_INVALID_SCOPE".to_string()))?
        .join(" ");
    let code = token::random_token();
    let approved = atp_oauth::approve_request(
        &state.db,
        &req.id,
        actor.id,
        &granted,
        &token::hash_secret(&code),
        Utc::now() + Duration::seconds(CODE_TTL_SECS),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("[atproto-oauth] {}", e)))?;
    if !approved {
        return Err(ApiError::NotFound("OAUTH_REQUEST_NOT_FOUND"));
    }
    Ok(Json(DecisionResponse {
        redirect_url: redirect_with(&state, &params, &[("code", &code)]),
    }))
}

/// `POST /api/atproto-oauth/deny` — 拒否して、クライアントへ `access_denied` を返す戻り先を返す。
pub async fn deny(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DecisionRequest>,
) -> Result<Json<DecisionResponse>, ApiError> {
    authed_actor(&state, &headers).await?;
    let (req, params) = load_pending(&state, &body.request_uri).await?;
    atp_oauth::delete_pending_request(&state.db, &req.id)
        .await
        .map_err(|e| ApiError::Internal(format!("[atproto-oauth] {}", e)))?;
    Ok(Json(DecisionResponse {
        redirect_url: redirect_with(
            &state,
            &params,
            &[
                ("error", "access_denied"),
                ("error_description", "The user denied the request"),
            ],
        ),
    }))
}

// ─── トークン ────────────────────────────────────────────────────────────────

/// `POST /oauth/atproto/token`
pub async fn token_endpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = token_inner(&state, &headers, &body).await;
    with_nonce(&state, result)
}

async fn token_inner(
    state: &AppState,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Response, OAuthError> {
    let jkt = verify_endpoint_dpop(state, headers, PATH_TOKEN).await?;
    let params = parse_params(headers, body)?;
    let (client, kid) = authenticate_client(state, &params).await?;
    match param(&params, "grant_type") {
        Some("authorization_code") => {
            exchange_code(state, &params, &client, kid.as_deref(), &jkt).await
        }
        Some("refresh_token") => refresh(state, &params, &client, kid.as_deref(), &jkt).await,
        _ => Err(OAuthError {
            status: StatusCode::BAD_REQUEST,
            error: "unsupported_grant_type",
            description: "grant_type must be authorization_code or refresh_token".to_string(),
        }),
    }
}

fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    is_code_challenge(verifier)
        && URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge
}

async fn exchange_code(
    state: &AppState,
    params: &HashMap<String, String>,
    client: &Client,
    kid: Option<&str>,
    jkt: &str,
) -> Result<Response, OAuthError> {
    let code =
        param(params, "code").ok_or_else(|| OAuthError::invalid_request("code is required"))?;
    // コードは照合の前に使い切る（失敗した交換で同じコードを試し直させない）。
    let req = atp_oauth::consume_code(&state.db, &token::hash_secret(code))
        .await
        .map_err(OAuthError::server_error)?
        .ok_or_else(|| OAuthError::invalid_grant("invalid or expired code"))?;
    let stored: StoredParams =
        serde_json::from_value(req.params.clone()).map_err(OAuthError::server_error)?;
    if req.client_id != client.client_id {
        return Err(OAuthError::invalid_grant(
            "code was issued to another client",
        ));
    }
    if param(params, "redirect_uri") != Some(stored.redirect_uri.as_str()) {
        return Err(OAuthError::invalid_grant("redirect_uri mismatch"));
    }
    if !param(params, "code_verifier").is_some_and(|v| pkce_matches(v, &stored.code_challenge)) {
        return Err(OAuthError::invalid_grant("invalid code_verifier"));
    }
    if req.dpop_jkt != jkt {
        return Err(OAuthError::invalid_grant(
            "DPoP key differs from the authorization request",
        ));
    }
    if req.client_kid.as_deref() != kid {
        return Err(OAuthError::invalid_grant(
            "client key differs from the authorization request",
        ));
    }
    let (Some(actor_id), Some(scope)) = (req.actor_id, req.granted_scope.clone()) else {
        return Err(OAuthError::invalid_grant("request was not approved"));
    };
    let did = actor_did(state, actor_id).await?;

    let refresh_token = token::random_token();
    let session_id = generate_snowflake_id(Utc::now());
    let expires_at = Utc::now()
        + if client.is_confidential() {
            Duration::days(CONFIDENTIAL_SESSION_TTL_DAYS)
        } else {
            Duration::days(PUBLIC_SESSION_TTL_DAYS)
        };
    let client_name = req.client_name.clone().unwrap_or_else(|| client.host());
    atp_oauth::insert_session(
        &state.db,
        &NewOAuthSession {
            id: session_id,
            actor_id,
            client_id: &client.client_id,
            client_name: &client_name,
            scope: &scope,
            dpop_jkt: jkt,
            client_kid: kid,
            confidential: client.is_confidential(),
            refresh_token_hash: &token::hash_secret(&refresh_token),
            expires_at,
        },
    )
    .await
    .map_err(OAuthError::server_error)?;
    token_response(state, &did, &scope, session_id, jkt, &refresh_token)
}

async fn refresh(
    state: &AppState,
    params: &HashMap<String, String>,
    client: &Client,
    kid: Option<&str>,
    jkt: &str,
) -> Result<Response, OAuthError> {
    let old = param(params, "refresh_token")
        .ok_or_else(|| OAuthError::invalid_request("refresh_token is required"))?;
    let old_hash = token::hash_secret(old);
    let new_token = token::random_token();
    let rotated = atp_oauth::rotate_refresh_token(
        &state.db,
        &RefreshRotation {
            old_hash: &old_hash,
            new_hash: &token::hash_secret(&new_token),
            client_id: &client.client_id,
            dpop_jkt: jkt,
            client_kid: kid,
            confidential_expires_at: Utc::now() + Duration::days(CONFIDENTIAL_SESSION_TTL_DAYS),
        },
    )
    .await
    .map_err(OAuthError::server_error)?;
    let Some(session) = rotated else {
        // 1つ前のトークンの再利用なら、漏れたものとしてセッションごと止める。
        if atp_oauth::revoke_session_by_reused_refresh_token(&state.db, &old_hash)
            .await
            .map_err(OAuthError::server_error)?
        {
            tracing::warn!("[atproto-oauth] 使用済みのリフレッシュトークンが再び使われたためセッションを失効させました");
        }
        return Err(OAuthError::invalid_grant("invalid refresh_token"));
    };
    let did = actor_did(state, session.actor_id).await?;
    token_response(state, &did, &session.scope, session.id, jkt, &new_token)
}

async fn actor_did(state: &AppState, actor_id: i64) -> Result<String, OAuthError> {
    state
        .actors
        .find_by_id(actor_id)
        .await
        .map_err(OAuthError::server_error)?
        .and_then(|a| a.at_did)
        .ok_or_else(|| OAuthError::invalid_grant("account is not available"))
}

fn token_response(
    state: &AppState,
    did: &str,
    scope: &str,
    session_id: i64,
    jkt: &str,
    refresh_token: &str,
) -> Result<Response, OAuthError> {
    let access_token = token::issue_access_token(
        &state.secrets.jwt_secret_bytes(),
        &NewAccessToken {
            issuer: &issuer(state),
            did,
            scope,
            session_id,
            jkt,
        },
    )
    .map_err(OAuthError::server_error)?;
    let mut res = Json(serde_json::json!({
        "access_token": access_token,
        "token_type": "DPoP",
        "expires_in": token::ACCESS_TOKEN_TTL_SECS,
        "refresh_token": refresh_token,
        "scope": scope,
        "sub": did,
    }))
    .into_response();
    res.headers_mut()
        .insert("cache-control", "no-store".parse().expect("static"));
    Ok(res)
}

// ─── 失効 ────────────────────────────────────────────────────────────────────

/// `POST /oauth/atproto/revoke`（RFC 7009）。トークンを持っていること自体を本人の証とし、
/// 知らないトークンでも 200 を返す（存否を漏らさない）。
pub async fn revoke(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let result = async {
        let params = parse_params(&headers, &body)?;
        let Some(t) = param(&params, "token") else {
            return Err(OAuthError::invalid_request("token is required"));
        };
        let revoked = atp_oauth::revoke_session_by_refresh_token(&state.db, &token::hash_secret(t))
            .await
            .map_err(OAuthError::server_error)?;
        if !revoked {
            if let Some(access) =
                token::verify_access_token(&state.secrets.jwt_secret_bytes(), &issuer(&state), t)
            {
                atp_oauth::revoke_session(&state.db, access.session_id)
                    .await
                    .map_err(OAuthError::server_error)?;
            }
        }
        Ok(Json(serde_json::json!({})).into_response())
    }
    .await;
    with_nonce(&state, result)
}

// ─── 設定画面 ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct OAuthSessionResponse {
    id: String,
    client_id: String,
    client_host: String,
    client_name: String,
    scopes: Vec<String>,
    created_at: chrono::DateTime<Utc>,
}

/// `GET /api/account/atproto-oauth-sessions` — 連携中の OAuth クライアント。
pub async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<OAuthSessionResponse>>, ApiError> {
    let actor = authed_actor(&state, &headers).await?;
    let rows = atp_oauth::list_active_sessions(&state.db, actor.id)
        .await
        .map_err(|e| ApiError::Internal(format!("[atproto-oauth] {}", e)))?;
    Ok(Json(
        rows.into_iter()
            .map(|r| OAuthSessionResponse {
                id: r.id.to_string(),
                client_host: url::Url::parse(&r.client_id)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                    .unwrap_or_default(),
                client_id: r.client_id,
                client_name: r.client_name,
                scopes: client::split_scope(&r.scope),
                created_at: r.created_at,
            })
            .collect(),
    ))
}

/// `DELETE /api/account/atproto-oauth-sessions/:id`
pub async fn revoke_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<()>, ApiError> {
    let actor = authed_actor(&state, &headers).await?;
    let revoked = atp_oauth::revoke_session_of_actor(&state.db, actor.id, id)
        .await
        .map_err(|e| ApiError::Internal(format!("[atproto-oauth] {}", e)))?;
    if !revoked {
        return Err(ApiError::NotFound("OAUTH_SESSION_NOT_FOUND"));
    }
    Ok(Json(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_s256() {
        // RFC 7636 付録 B の例
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert!(pkce_matches(
            verifier,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
        assert!(!pkce_matches(
            verifier,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cX"
        ));
        assert!(!pkce_matches(
            "short",
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        ));
    }
}
