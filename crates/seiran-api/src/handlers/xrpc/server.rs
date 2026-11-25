use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use sha2::Digest;

use seiran_common::atp::resolve_external_handle;
use seiran_common::LocalAuthProvider;

use super::{extract_bearer, require_privileged_session, service_did};
use crate::error::ApiError;
use crate::handlers::app_passwords;
use crate::middleware::ClientIp;
use crate::rate_limit::{self, AttemptKind};
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

    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "NotFound", "message": "ハンドルが見つかりません"})),
        )
            .into_response()
    };

    // {username}.{local_domain} 形式でなければ自ドメイン管理外。
    // DNS TXT / HTTP well-known 経由で解決を試みる（bsky.app 等が任意ハンドルの
    // resolveHandle をログイン中PDSに投げてくるため、即400で拒否すると呼び出し元が壊れる）。
    let suffix = format!(".{}", state.local_domain);
    let Some(username) = handle.strip_suffix(&suffix) else {
        return match resolve_external_handle(&handle, &state.http_client).await {
            Some(did) => Json(serde_json::json!({"did": did})).into_response(),
            None => not_found(),
        };
    };

    if username.is_empty() || username.contains('.') {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "InvalidRequest", "message": "無効なハンドルです"})),
        )
            .into_response();
    }

    match state
        .actors
        .find_did_by_username_domain(username, &state.local_domain)
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
    /// メール2FA確認コード。SMTP設定済みインスタンスでは必須（`auth_factor_token_required_error`
    /// を返しコード送信、2回目の呼び出しで検証）。SMTP未設定インスタンスでは常にスキップする。
    #[serde(rename = "authFactorToken")]
    pub auth_factor_token: Option<String>,
}

fn auth_factor_token_required_error() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "AuthFactorTokenRequired", "message": "メールで送信した確認コードを入力してください"})),
    )
        .into_response()
}

/// `com.atproto.server.createSession` — ハンドルまたはDID + パスワードでログインし、
/// accessJwt/refreshJwt を発行する。本アカウントのメインパスワードと、`createAppPassword`
/// で発行した専用アプリパスワードの両方を照合対象とする（公式Bluesky PDS準拠）。
/// アカウント存在有無が応答の分岐やタイミングから漏れないよう、identifier解決失敗時も
/// ダミーハッシュ照合を行ってから同一の `AuthenticationRequired` を返す。
/// ATP クライアントからの操作で送るメールの言語。クライアントは画面の言語を送ってこないので、
/// seiran で選んだ表示言語（`users.language_preference`）を優先し、「自動」なら
/// `Accept-Language`（アプリが OS の言語を載せる）から決める。
pub(crate) async fn atp_mail_language(
    state: &AppState,
    user_id: i64,
    headers: &HeaderMap,
) -> &'static str {
    let preference = state
        .users
        .find_language_preference_by_user_id(user_id)
        .await
        .ok()
        .flatten();
    crate::i18n::request_language(preference.as_deref(), headers)
}

pub async fn xrpc_create_session(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    // [SEC-1] メインパスワードでの認証も受け付けるため（下記コメント参照）、
    // /api/auth/login と同じ総当たり対策を通す。login 側と同じ AttemptKind::Login
    // バケットを共有し、identifier を変えた試行の合算でも検知できるようにする。
    if let Err(e) = rate_limit::check_ip_not_blocked(&state, &ip).await {
        return e.into_response();
    }

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
        // ユーザーが実在しない場合も試行として記録する（login と同じ扱い）。
        if let Err(e) = rate_limit::check_and_record_credential_attempt(
            &state,
            AttemptKind::Login,
            &ip,
            identifier,
            &req.password,
            None,
        )
        .await
        {
            return e.into_response();
        }
        let _ = LocalAuthProvider::verify_password(&req.password, LocalAuthProvider::dummy_hash());
        return auth_required_error();
    };
    let did = actor.at_did.clone().expect("filter済み");

    let window_reset_at = match actor.user_id {
        Some(user_id) => rate_limit::window_reset_at(&state, user_id).await,
        None => None,
    };
    if let Err(e) = rate_limit::check_and_record_credential_attempt(
        &state,
        AttemptKind::Login,
        &ip,
        identifier,
        &req.password,
        window_reset_at,
    )
    .await
    {
        return e.into_response();
    }

    let app_passwords = match state.atp_sessions.find_active_app_passwords(actor.id).await {
        Ok(rows) => rows,
        Err(e) => {
            return ApiError::Internal(format!("[createSession] DB エラー: {}", e)).into_response()
        }
    };
    let matched_app_password_id = app_passwords
        .iter()
        .find(|p| {
            LocalAuthProvider::verify_password(&req.password, &p.password_hash).unwrap_or(false)
        })
        .map(|p| p.id);

    // 本アカウントのメインパスワードでもログインできる（公式Bluesky PDS準拠。bsky.app自身も
    // メインパスワードでcreateSessionを呼ぶ）。どちらで認証したかはセッションの`privileged`に
    // 残し、アプリパスワードのセッションにはDID移行等の権限の強い操作をさせない。
    let login_row = state
        .users
        .find_login_by_username(&actor.username)
        .await
        .ok()
        .flatten();
    let main_hash = login_row.as_ref().and_then(|l| l.password_hash.clone());
    let main_password_ok = match &main_hash {
        Some(h) => LocalAuthProvider::verify_password(&req.password, h).unwrap_or(false),
        None => {
            let _ =
                LocalAuthProvider::verify_password(&req.password, LocalAuthProvider::dummy_hash());
            false
        }
    };
    if !main_password_ok && matched_app_password_id.is_none() {
        return auth_required_error();
    }
    // メインパスワードと一致したならメインパスワードでのログインとして扱う（アプリパスワードの
    // 失効に巻き込まない）。
    let app_password_id = if main_password_ok {
        None
    } else {
        matched_app_password_id
    };

    // メール2FA。SMTP設定済みインスタンスのみ有効（コード送信自体が不可能なインスタンスでは
    // 常にスキップする）。標準ATPクライアント（Bluesky公式アプリ等）の`authFactorToken`UIは
    // 「メールを確認してください」という文言のため、TOTPではなくメール送信コードで統一する。
    // アプリパスワードでのログインには求めない（公式PDSと同じ）。アプリパスワードは本人が
    // seiranにログインして発行したランダムな値で、確認コードの入力欄を持たないクライアント
    // （アプリパスワード前提のサードパーティクライアント）でも使えるようにするためのもの。
    let smtp_settings = state.site_settings.get_all().await.unwrap_or_default();
    if main_password_ok && crate::mailer::is_smtp_configured(&smtp_settings) {
        const PURPOSE: &str = "atp_session_2fa";
        // bsky.app はコード入力欄を出す前の初回ログインでも `authFactorToken: ""` を送る。
        // 空文字を「コード入力あり」と扱うと照合に失敗して AuthenticationRequired になり、
        // コード入力欄に進めないままログインできなくなるため、未指定と同じに扱う（公式PDS準拠）。
        match req
            .auth_factor_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            None => {
                let code = format!("{:06}", uuid::Uuid::new_v4().as_u128() % 1_000_000);
                let code_hash = hex::encode(sha2::Sha256::digest(code.as_bytes()));
                let now = chrono::Utc::now();
                let id = seiran_common::generate_snowflake_id(now);
                if let Err(e) = state
                    .email_short_codes
                    .issue(
                        id,
                        actor.id,
                        PURPOSE,
                        &code_hash,
                        now + chrono::Duration::minutes(15),
                        now,
                    )
                    .await
                {
                    return ApiError::Internal(format!(
                        "[createSession] 確認コード発行失敗: {}",
                        e
                    ))
                    .into_response();
                }
                if let Some(login) = &login_row {
                    let language = atp_mail_language(&state, login.id, &headers).await;
                    if let Err(e) = crate::mailer::send_atp_session_2fa_code(
                        &smtp_settings,
                        language,
                        &login.email,
                        &code,
                    )
                    .await
                    {
                        tracing::error!("[createSession] 確認コード送信失敗: {}", e);
                    }
                }
                return auth_factor_token_required_error();
            }
            Some(token) => {
                let code_hash = hex::encode(sha2::Sha256::digest(token.as_bytes()));
                match state
                    .email_short_codes
                    .consume(actor.id, PURPOSE, &code_hash)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => return auth_required_error(),
                    Err(e) => {
                        return ApiError::Internal(format!(
                            "[createSession] 確認コード検証失敗: {}",
                            e
                        ))
                        .into_response()
                    }
                }
            }
        }
    }

    let (access_jwt, refresh_jwt, jti, refresh_exp) =
        match state
            .local_auth
            .generate_atp_session(&did, &service_did(&state), main_password_ok)
        {
            Ok(t) => t,
            Err(e) => {
                return ApiError::Internal(format!("[createSession] トークン発行失敗: {}", e))
                    .into_response()
            }
        };
    if let Err(e) = state
        .atp_sessions
        .insert_refresh_token(jti, actor.id, app_password_id, refresh_exp)
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
        // 標準ATPクライアント互換、および他PDSからの既存DID転入（`docs/account_migration.md`）が
        // このメールをそのまま信頼して使う。seiranはアカウント単位の「確認済み」フラグを
        // 別途持たないため（`email_verifications`は登録時点の一度きりのトークン消費であり
        // 継続的な状態ではない）、メール登録済みのアカウントは常にconfirmed扱いとする。
        "email": login_row.as_ref().map(|l| l.email.clone()),
        "emailConfirmed": login_row.is_some(),
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
    let owner = match state.atp_sessions.consume_refresh_token(verified.jti).await {
        Ok(Some(owner)) => owner,
        Ok(None) => return ApiError::Unauthorized("トークンが無効です").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[refreshSession] DB エラー: {}", e)).into_response()
        }
    };
    let actor_id = owner.actor_id;

    let actor = match state.actors.find_by_id(actor_id).await {
        Ok(Some(a)) => a,
        _ => return ApiError::Unauthorized("アクターが見つかりません").into_response(),
    };
    let Some(did) = actor.at_did.clone() else {
        return ApiError::Unauthorized("アクターが見つかりません").into_response();
    };

    let (access_jwt, refresh_jwt, new_jti, refresh_exp) = match state
        .local_auth
        .generate_atp_session(&did, &service_did(&state), verified.privileged)
    {
        Ok(t) => t,
        Err(e) => {
            return ApiError::Internal(format!("[refreshSession] トークン発行失敗: {}", e))
                .into_response()
        }
    };
    if let Err(e) = state
        .atp_sessions
        .insert_refresh_token(new_jti, actor_id, owner.app_password_id, refresh_exp)
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

/// `com.atproto.server.checkAccountStatus` — 転出先PDSが移行準備状況を確認するために
/// 呼ぶ、読み取りのみのステータス。認可パターンは`xrpc_get_session`と同じ
/// （トークンから解決したDIDのアカウントについてのみ返す）。
pub async fn xrpc_check_account_status(
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

    let activated = actor.withdrawn_at.is_none()
        && actor.suspended_at.is_none()
        && actor.did_moved_out_at.is_none();

    let (indexed_records, repo_blocks, blobs) =
        match seiran_common::repository::atp::account_status_counts(&state.db, actor.id).await {
            Ok(v) => v,
            Err(e) => {
                return ApiError::Internal(format!("[checkAccountStatus] 件数の集計失敗: {}", e))
                    .into_response()
            }
        };

    Json(serde_json::json!({
        "activated": activated,
        "validDid": true,
        "repoCommit": actor.at_repo_cid,
        "repoRev": actor.at_repo_rev,
        "repoBlocks": repo_blocks,
        "indexedRecords": indexed_records,
        "privateStateValues": 0,
        "expectedBlobs": blobs,
        "importedBlobs": blobs,
    }))
    .into_response()
}

/// `com.atproto.server.deactivateAccount` — 転出先PDSが転入完了後（またはユーザーが
/// 転入を中断した際）のベストエフォート後始末として呼ぶ。`submitPlcOperation`の有無に
/// かかわらず、呼ばれたら`did_moved_out_at`をセットする（DIDが実際に転出したかに関わらず
/// 「seiran上でのこのアカウントの通常利用は終わり」という状態に統一する。4つ目の状態を
/// 新設しない設計判断）。
pub async fn xrpc_deactivate_account(
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
    if let Err(e) = require_privileged_session(&verified) {
        return e.into_response();
    }
    let actor = match state.actors.find_by_did(&verified.did).await {
        Ok(Some(a)) => a,
        _ => return ApiError::Unauthorized("アクターが見つかりません").into_response(),
    };

    if let Err(e) = seiran_common::repository::actor::mark_did_moved_out(
        &state.db,
        actor.id,
        chrono::Utc::now(),
    )
    .await
    {
        return ApiError::Internal(format!("[deactivateAccount] DB更新失敗: {}", e))
            .into_response();
    }
    tracing::warn!(
        "[deactivateAccount] actor_id={} did={} did_moved_out_atを設定",
        actor.id,
        verified.did
    );

    Json(serde_json::json!({})).into_response()
}

#[derive(Deserialize)]
pub struct CreateAppPasswordRequest {
    pub name: String,
}

/// `com.atproto.server.createAppPassword` — 新しいアプリパスワードを発行する。認証は seiran の
/// 通常ログインか、メインパスワードの ATP セッション（`app_passwords::authenticate_manager`）。
/// 平文パスワードはこのレスポンス以外では二度と取得できない。
pub async fn xrpc_create_app_password(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateAppPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = app_passwords::authenticate_manager(&state, &headers).await?;
    let created = app_passwords::create(&state, &actor, &req.name).await?;
    Ok(Json(serde_json::json!({
        "name": created.name,
        "password": created.password,
        "createdAt": created.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "privileged": false,
    })))
}

/// `com.atproto.server.listAppPasswords` — 発行済みアプリパスワード一覧（名前・作成日時のみ）。
pub async fn xrpc_list_app_passwords(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = app_passwords::authenticate_manager(&state, &headers).await?;
    let rows = app_passwords::list(&state, &actor).await?;
    Ok(Json(serde_json::json!({
        "passwords": rows.into_iter().map(|r| serde_json::json!({
            "name": r.name,
            "createdAt": r.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "privileged": false,
        })).collect::<Vec<_>>()
    })))
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
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = app_passwords::authenticate_manager(&state, &headers).await?;
    app_passwords::revoke_by_name(&state, &actor, &req.name).await?;
    Ok(Json(serde_json::json!({})))
}
