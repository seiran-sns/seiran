use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;

use seiran_common::{generate_snowflake_id, LocalAuthProvider};

use super::{extract_bearer, service_did};
use crate::error::ApiError;
use crate::middleware::extract_auth;
use crate::AppState;

#[derive(Deserialize)]
pub struct ResolveHandleQuery {
    pub handle: String,
}

fn auth_required_error() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "AuthenticationRequired", "message": "識別子またはパスワードが正しくありません"})),
    )
        .into_response()
}

pub async fn xrpc_describe_server(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "did": format!("did:web:{}", state.local_domain),
        "availableUserDomains": [state.local_domain.as_str()],
        "inviteCodeRequired": false,
        "phoneVerificationRequired": false,
    }))
}

pub async fn xrpc_resolve_handle(
    Query(params): Query<ResolveHandleQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let handle = params.handle.trim().to_lowercase();

    // {username}.{local_domain} 形式かチェック
    let suffix = format!(".{}", state.local_domain);
    let username = if let Some(u) = handle.strip_suffix(&suffix) {
        u.to_string()
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "InvalidRequest", "message": "このPDSが管理していないハンドルです"})),
        ).into_response();
    };

    if username.is_empty() || username.contains('.') {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "InvalidRequest", "message": "無効なハンドルです"})),
        )
            .into_response();
    }

    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "NotFound", "message": "ハンドルが見つかりません"})),
        )
            .into_response()
    };

    match state
        .actors
        .find_did_by_username_domain(&username, &state.local_domain)
        .await
    {
        Ok(Some(did)) if !did.is_empty() => Json(serde_json::json!({"did": did})).into_response(),
        Ok(_) => not_found(),
        Err(e) => {
            tracing::error!("[resolveHandle] DB エラー: {}", e);
            not_found()
        }
    }
}

pub async fn well_known_did(State(state): State<AppState>) -> impl IntoResponse {
    let did = format!("did:web:{}", state.local_domain);
    let endpoint = format!("https://{}", state.local_domain);
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Json(serde_json::json!({
            "@context": ["https://www.w3.org/ns/did/v1"],
            "id": did,
            "service": [
                {
                    "id": "#atproto_pds",
                    "type": "AtprotoPersonalDataServer",
                    "serviceEndpoint": endpoint,
                }
            ]
        })),
    )
}

pub async fn well_known_atproto_did(
    axum::extract::Host(host): axum::extract::Host,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let username = host.split('.').next().unwrap_or("").to_string();

    if username.is_empty() || username == state.local_domain {
        return ApiError::NotFound("").into_response();
    }

    match state
        .actors
        .find_did_by_username_domain(&username, &state.local_domain)
        .await
    {
        Ok(Some(did)) if !did.is_empty() => {
            ([(axum::http::header::CONTENT_TYPE, "text/plain")], did).into_response()
        }
        Ok(_) => ApiError::NotFound("").into_response(),
        Err(e) => {
            ApiError::Internal(format!("[well_known_atproto_did] DB エラー: {}", e)).into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct CreateSessionRequest {
    pub identifier: String,
    pub password: String,
}

/// `com.atproto.server.createSession` — ハンドルまたはDID + アプリパスワードでログインし、
/// accessJwt/refreshJwt を発行する。本アカウントのメインパスワードは受け付けない
/// （`createAppPassword` で発行した専用パスワードのみ照合対象）。
/// アカウント存在有無が応答の分岐やタイミングから漏れないよう、identifier解決失敗時も
/// ダミーハッシュ照合を行ってから同一の `AuthenticationRequired` を返す。
pub async fn xrpc_create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let identifier = req.identifier.trim();

    let actor = match state.actors.find_by_did(identifier).await {
        Ok(Some(a)) => Some(a),
        _ => {
            let suffix = format!(".{}", state.local_domain);
            let username = identifier.strip_suffix(&suffix).unwrap_or(identifier);
            state
                .actors
                .find_by_username_domain(username, &state.local_domain)
                .await
                .ok()
                .flatten()
        }
    };

    let Some(actor) = actor.filter(|a| a.at_did.is_some()) else {
        let _ = LocalAuthProvider::verify_password(&req.password, LocalAuthProvider::dummy_hash());
        return auth_required_error();
    };
    let did = actor.at_did.clone().expect("filter済み");

    let hashes = match state
        .atp_sessions
        .find_active_password_hashes(actor.id)
        .await
    {
        Ok(h) => h,
        Err(e) => {
            return ApiError::Internal(format!("[createSession] DB エラー: {}", e)).into_response()
        }
    };
    let password_ok = hashes
        .iter()
        .any(|h| LocalAuthProvider::verify_password(&req.password, h).unwrap_or(false));
    if hashes.is_empty() {
        let _ = LocalAuthProvider::verify_password(&req.password, LocalAuthProvider::dummy_hash());
    }
    if !password_ok {
        return auth_required_error();
    }

    let (access_jwt, refresh_jwt, jti, refresh_exp) = match state
        .local_auth
        .generate_atp_session(&did, &service_did(&state))
    {
        Ok(t) => t,
        Err(e) => {
            return ApiError::Internal(format!("[createSession] トークン発行失敗: {}", e))
                .into_response()
        }
    };
    if let Err(e) = state
        .atp_sessions
        .insert_refresh_token(jti, actor.id, refresh_exp)
        .await
    {
        return ApiError::Internal(format!(
            "[createSession] リフレッシュトークン記録失敗: {}",
            e
        ))
        .into_response();
    }

    Json(serde_json::json!({
        "did": did,
        "handle": format!("{}.{}", actor.username, state.local_domain),
        "accessJwt": access_jwt,
        "refreshJwt": refresh_jwt,
        "active": true,
    }))
    .into_response()
}

/// `com.atproto.server.refreshSession` — refreshJwt を検証し、新しいaccessJwt/refreshJwtの
/// ペアを発行する。古いrefreshJwtの `jti` は同時に失効させる（ワンタイム・ローテーション）。
pub async fn xrpc_refresh_session(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let Some(token) = extract_bearer(&headers) else {
        return ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response();
    };
    let verified = match state
        .local_auth
        .verify_atp_refresh_token(token, &service_did(&state))
    {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("トークンが無効です").into_response(),
    };
    let actor_id = match state
        .atp_sessions
        .find_valid_refresh_token_actor(verified.jti)
        .await
    {
        Ok(Some(id)) => id,
        Ok(None) => return ApiError::Unauthorized("トークンが無効です").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[refreshSession] DB エラー: {}", e)).into_response()
        }
    };
    if let Err(e) = state.atp_sessions.revoke_refresh_token(verified.jti).await {
        return ApiError::Internal(format!("[refreshSession] 失効処理失敗: {}", e)).into_response();
    }

    let actor = match state.actors.find_by_id(actor_id).await {
        Ok(Some(a)) => a,
        _ => return ApiError::Unauthorized("アクターが見つかりません").into_response(),
    };
    let Some(did) = actor.at_did.clone() else {
        return ApiError::Unauthorized("アクターが見つかりません").into_response();
    };

    let (access_jwt, refresh_jwt, new_jti, refresh_exp) = match state
        .local_auth
        .generate_atp_session(&did, &service_did(&state))
    {
        Ok(t) => t,
        Err(e) => {
            return ApiError::Internal(format!("[refreshSession] トークン発行失敗: {}", e))
                .into_response()
        }
    };
    if let Err(e) = state
        .atp_sessions
        .insert_refresh_token(new_jti, actor_id, refresh_exp)
        .await
    {
        return ApiError::Internal(format!(
            "[refreshSession] リフレッシュトークン記録失敗: {}",
            e
        ))
        .into_response();
    }

    Json(serde_json::json!({
        "did": did,
        "handle": format!("{}.{}", actor.username, state.local_domain),
        "accessJwt": access_jwt,
        "refreshJwt": refresh_jwt,
        "active": true,
    }))
    .into_response()
}

/// `com.atproto.server.deleteSession` — refreshJwt の `jti` を失効させる（ログアウト）。
pub async fn xrpc_delete_session(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let Some(token) = extract_bearer(&headers) else {
        return ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response();
    };
    let verified = match state
        .local_auth
        .verify_atp_refresh_token(token, &service_did(&state))
    {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("トークンが無効です").into_response(),
    };
    if let Err(e) = state.atp_sessions.revoke_refresh_token(verified.jti).await {
        return ApiError::Internal(format!("[deleteSession] 失効処理失敗: {}", e)).into_response();
    }
    Json(serde_json::json!({})).into_response()
}

/// `com.atproto.server.getSession` — accessJwt を検証し、現在のセッション情報を返す。
pub async fn xrpc_get_session(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let Some(token) = extract_bearer(&headers) else {
        return ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response();
    };
    let verified = match state
        .local_auth
        .verify_atp_access_token(token, &service_did(&state))
    {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("トークンが無効です").into_response(),
    };
    let actor = match state.actors.find_by_did(&verified.did).await {
        Ok(Some(a)) => a,
        _ => return ApiError::Unauthorized("アクターが見つかりません").into_response(),
    };

    Json(serde_json::json!({
        "did": verified.did,
        "handle": format!("{}.{}", actor.username, state.local_domain),
        "active": true,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct CreateAppPasswordRequest {
    pub name: String,
}

/// `com.atproto.server.createAppPassword` — 既存のseiranログイン（設定画面のアプリトークンと
/// 同様、`Authorization: Bearer` の自社トークン）で認証したユーザーが、自分のATPアプリ
/// パスワードを新規発行する。平文パスワードはこのレスポンス以外では二度と取得できない。
pub async fn xrpc_create_app_password(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateAppPasswordRequest>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let actor = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a,
        _ => return ApiError::NotFound("アクターが見つかりません").into_response(),
    };
    if actor.at_did.is_none() {
        return ApiError::BadRequest("ATPアカウントが未設定です".to_string()).into_response();
    }
    let name = req.name.trim();
    if name.is_empty() {
        return ApiError::BadRequest("name が必要です".to_string()).into_response();
    }

    let password = LocalAuthProvider::generate_app_password();
    let hash = match LocalAuthProvider::hash_password(&password) {
        Ok(h) => h,
        Err(e) => {
            return ApiError::Internal(format!("[createAppPassword] ハッシュ化失敗: {}", e))
                .into_response()
        }
    };
    let id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = state
        .atp_sessions
        .insert_app_password(id, actor.id, name, &hash)
        .await
    {
        return ApiError::Internal(format!("[createAppPassword] DB エラー: {}", e)).into_response();
    }

    Json(serde_json::json!({
        "name": name,
        "password": password,
        "createdAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    }))
    .into_response()
}

/// `com.atproto.server.listAppPasswords` — 発行済みアプリパスワード一覧（名前・作成日時のみ）。
pub async fn xrpc_list_app_passwords(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let actor = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a,
        _ => return ApiError::NotFound("アクターが見つかりません").into_response(),
    };
    match state.atp_sessions.list_app_passwords(actor.id).await {
        Ok(rows) => Json(serde_json::json!({
            "passwords": rows.into_iter().map(|r| serde_json::json!({
                "name": r.name,
                "createdAt": r.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            })).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(e) => {
            ApiError::Internal(format!("[listAppPasswords] DB エラー: {}", e)).into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct RevokeAppPasswordRequest {
    pub name: String,
}

/// `com.atproto.server.revokeAppPassword` — 名前指定でアプリパスワードを無効化する。
pub async fn xrpc_revoke_app_password(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<RevokeAppPasswordRequest>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let actor = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a,
        _ => return ApiError::NotFound("アクターが見つかりません").into_response(),
    };
    match state
        .atp_sessions
        .revoke_app_password(actor.id, &req.name)
        .await
    {
        Ok(true) => Json(serde_json::json!({})).into_response(),
        Ok(false) => ApiError::NotFound("アプリパスワードが見つかりません").into_response(),
        Err(e) => {
            ApiError::Internal(format!("[revokeAppPassword] DB エラー: {}", e)).into_response()
        }
    }
}
