//! Mastodon 互換の OAuth 2.0（Authorization Code フロー、PKCE 対応）。
//!
//! 1. `POST /api/v1/apps` — クライアントが自分を登録し client_id/client_secret を得る。
//! 2. `GET /oauth/authorize` — ブラウザで開かれる。検証だけして SPA の承認画面
//!    （`/oauth-connect`）へ引き継ぐ（ログイン要否・承認操作は MiAuth と同じく SPA 側）。
//! 3. `POST /api/oauth/authorize` — SPA が通常の Bearer 認証で呼び、認可コードを発行して
//!    リダイレクト先を返す。
//! 4. `POST /oauth/token` — クライアントがコードをアクセストークンに引き換える。
//!
//! 発行するアクセストークンは MiAuth と同じ無期限 JWT（`generate_app_token`）で、既存の
//! `extract_auth` がそのまま検証する。`app_tokens` に記録するので、設定画面の連携アプリ
//! 一覧から無効化できる。scope は記録するが強制しない（MiAuth と同じく、アプリ単位の権限
//! 制限は未対応）。

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use seiran_common::repository::oauth::{
    self, random_token, sha256_hex, NewAuthorizationCode, NewOAuthApp, OAuthAppRow,
};

use crate::error::ApiError;
use crate::middleware::{extract_auth, AuthedUser};
use crate::AppState;

use super::extract::{lenient, MastodonParams};
use super::types::{MastodonApplication, MastodonToken};

/// ブラウザ外でコードを表示させる特別な redirect_uri（RFC 8252 以前の慣習、Mastodon も対応）。
const OOB_REDIRECT_URI: &str = "urn:ietf:wg:oauth:2.0:oob";

/// 認可コードの有効期間（Mastodon 本家と同じ10分）。
const CODE_TTL_MINUTES: i64 = 10;

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

/// OAuth のエラー応答（RFC 6749 5.2 の `{"error", "error_description"}`）。
fn oauth_error(status: StatusCode, error: &'static str, description: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

/// 登録を受け付ける redirect_uri か。`https`（ホストは問わない）、ループバックの `http`
/// （RFC 8252 7.3、デスクトップアプリ用）、ネイティブアプリのカスタムスキーム、OOB を許す。
/// 平文 `http` の外部ホストは認可コードが漏れるため、スクリプト実行系スキームはリダイレクトで
/// 任意コードを実行させられるため拒否する。
fn is_valid_redirect_uri(uri: &str) -> bool {
    if uri == OOB_REDIRECT_URI {
        return true;
    }
    let Ok(parsed) = url::Url::parse(uri) else {
        return false;
    };
    match parsed.scheme() {
        "https" => parsed.host_str().is_some(),
        "http" => matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        "javascript" | "data" | "vbscript" | "file" | "blob" => false,
        _ => true,
    }
}

/// `redirect_uris` はスペース・改行区切りの文字列でも配列でも送られてくる。
fn split_redirect_uris(raw: Vec<String>) -> Vec<String> {
    raw.iter()
        .flat_map(|s| s.split_whitespace())
        .map(str::to_owned)
        .collect()
}

fn scopes_vec(scopes: &str) -> Vec<String> {
    scopes.split_whitespace().map(str::to_owned).collect()
}

fn to_application(app: &OAuthAppRow) -> MastodonApplication {
    MastodonApplication {
        id: app.id.to_string(),
        name: app.name.clone(),
        website: app.website.clone(),
        scopes: scopes_vec(&app.scopes),
        redirect_uri: app.redirect_uris.join("\n"),
        redirect_uris: app.redirect_uris.clone(),
        client_id: None,
        client_secret: None,
        vapid_key: None,
    }
}

#[derive(Deserialize)]
pub struct CreateAppParams {
    pub client_name: Option<String>,
    #[serde(default, deserialize_with = "lenient::vec_string")]
    pub redirect_uris: Vec<String>,
    pub scopes: Option<String>,
    pub website: Option<String>,
}

/// POST /api/v1/apps
pub async fn create_app(
    State(state): State<AppState>,
    MastodonParams(params): MastodonParams<CreateAppParams>,
) -> Result<Json<MastodonApplication>, ApiError> {
    let name = params
        .client_name
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| ApiError::BadRequest("client_name is required".to_owned()))?;
    let redirect_uris = split_redirect_uris(params.redirect_uris);
    if redirect_uris.is_empty() || !redirect_uris.iter().all(|u| is_valid_redirect_uri(u)) {
        return Err(ApiError::BadRequest("invalid redirect_uris".to_owned()));
    }
    let scopes = params
        .scopes
        .map(|s| scopes_vec(&s).join(" "))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "read".to_owned());
    let website = params.website.filter(|w| !w.trim().is_empty());

    let id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    let client_id = random_token();
    let client_secret = random_token();
    oauth::insert_app(
        &state.db,
        &NewOAuthApp {
            id,
            client_id: &client_id,
            client_secret_hash: &sha256_hex(&client_secret),
            name: &name,
            redirect_uris: &redirect_uris,
            scopes: &scopes,
            website: website.as_deref(),
            description: None,
        },
    )
    .await
    .map_err(internal)?;

    Ok(Json(MastodonApplication {
        id: id.to_string(),
        name,
        website,
        scopes: scopes_vec(&scopes),
        redirect_uri: redirect_uris.join("\n"),
        redirect_uris,
        client_id: Some(client_id),
        client_secret: Some(client_secret),
        vapid_key: None,
    }))
}

/// GET /api/v1/apps/verify_credentials — トークンを発行したアプリの情報。OAuth 以外
/// （MiAuth・自社ログイン）のトークンでは対応するアプリが無いので 401。
pub async fn verify_app_credentials(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<MastodonApplication>, ApiError> {
    extract_auth(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized("The access token is invalid"))?;
    let verified = state
        .local_auth
        .verify_token_ignoring_exp(bearer)
        .map_err(|_| ApiError::Unauthorized("The access token is invalid"))?;
    let app = oauth::find_app_by_token_id(&state.db, verified.jti)
        .await
        .map_err(internal)?
        .ok_or(ApiError::Unauthorized("The access token is invalid"))?;
    Ok(Json(to_application(&app)))
}

/// 認可要求のパラメータ（`GET /oauth/authorize` のクエリ、SPA からの承認要求の両方）。
#[derive(Deserialize, Serialize, Clone)]
pub struct AuthorizeParams {
    pub response_type: Option<String>,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: Option<String>,
    pub state: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
}

/// クライアント登録と照合して、認可要求として正しいか検証する。
async fn validate_authorize(
    state: &AppState,
    params: &AuthorizeParams,
) -> Result<OAuthAppRow, ApiError> {
    if params.response_type.as_deref().is_some_and(|t| t != "code") {
        return Err(ApiError::BadRequest("unsupported_response_type".to_owned()));
    }
    if let Some(method) = params.code_challenge_method.as_deref() {
        if method != "S256" {
            return Err(ApiError::BadRequest("invalid_request".to_owned()));
        }
    }
    let app = oauth::find_app_by_client_id(&state.db, &params.client_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::BadRequest("invalid_client".to_owned()))?;
    if !app.redirect_uris.iter().any(|u| u == &params.redirect_uri) {
        return Err(ApiError::BadRequest("invalid_redirect_uri".to_owned()));
    }
    Ok(app)
}

/// GET /oauth/authorize — 検証してから SPA の承認画面へ同じクエリで引き継ぐ。
pub async fn authorize_page(
    State(state): State<AppState>,
    Query(params): Query<AuthorizeParams>,
) -> Result<Redirect, ApiError> {
    validate_authorize(&state, &params).await?;
    let query = serde_urlencoded_query(&params);
    Ok(Redirect::to(&format!("/oauth-connect?{query}")))
}

fn serde_urlencoded_query(params: &AuthorizeParams) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    s.append_pair("client_id", &params.client_id);
    s.append_pair("redirect_uri", &params.redirect_uri);
    for (k, v) in [
        ("response_type", &params.response_type),
        ("scope", &params.scope),
        ("state", &params.state),
        ("code_challenge", &params.code_challenge),
        ("code_challenge_method", &params.code_challenge_method),
    ] {
        if let Some(v) = v {
            s.append_pair(k, v);
        }
    }
    s.finish()
}

#[derive(Serialize)]
pub struct OAuthAppInfo {
    pub name: String,
    pub website: Option<String>,
}

/// GET /api/oauth/apps/:client_id — SPA の承認画面に出すアプリ名。URL のクエリに載った
/// 名前を表示すると、他人のアプリを騙る承認画面を作れてしまうため、登録内容から引く。
pub async fn app_info(
    State(state): State<AppState>,
    Path(client_id): Path<String>,
) -> Result<Json<OAuthAppInfo>, ApiError> {
    let app = oauth::find_app_by_client_id(&state.db, &client_id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound("OAUTH_APP_NOT_FOUND"))?;
    Ok(Json(OAuthAppInfo {
        name: app.name,
        website: app.website,
    }))
}

#[derive(Serialize)]
pub struct AuthorizeResponse {
    /// リダイレクト先（コード・state 付き）。OOB のときは `None` で、SPA が `code` を表示する。
    pub redirect_url: Option<String>,
    pub code: String,
}

/// `redirect_uri` に `code`・`state` を付ける（既存のクエリは保つ）。
fn redirect_with_code(redirect_uri: &str, code: &str, oauth_state: Option<&str>) -> String {
    if let Ok(mut url) = url::Url::parse(redirect_uri) {
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("code", code);
            if let Some(s) = oauth_state {
                q.append_pair("state", s);
            }
        }
        return url.to_string();
    }
    let sep = if redirect_uri.contains('?') { '&' } else { '?' };
    let mut out = format!("{redirect_uri}{sep}code={}", urlencoding::encode(code));
    if let Some(s) = oauth_state {
        out.push_str(&format!("&state={}", urlencoding::encode(s)));
    }
    out
}

/// POST /api/oauth/authorize — SPA の承認画面から（ログイン中ユーザーの Bearer 認証で）。
pub async fn authorize(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(params): Json<AuthorizeParams>,
) -> Result<Json<AuthorizeResponse>, ApiError> {
    let app = validate_authorize(&state, &params).await?;
    let scopes = params
        .scope
        .as_deref()
        .map(|s| scopes_vec(s).join(" "))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| app.scopes.clone());
    let code = random_token();
    oauth::insert_authorization_code(
        &state.db,
        &NewAuthorizationCode {
            code_hash: &sha256_hex(&code),
            app_id: app.id,
            user_id: user.user_id,
            redirect_uri: &params.redirect_uri,
            scopes: &scopes,
            code_challenge: params.code_challenge.as_deref(),
            code_challenge_method: params
                .code_challenge
                .as_ref()
                .map(|_| params.code_challenge_method.as_deref().unwrap_or("S256")),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(CODE_TTL_MINUTES),
        },
    )
    .await
    .map_err(internal)?;

    let redirect_url = (params.redirect_uri != OOB_REDIRECT_URI)
        .then(|| redirect_with_code(&params.redirect_uri, &code, params.state.as_deref()));
    Ok(Json(AuthorizeResponse { redirect_url, code }))
}

#[derive(Deserialize)]
pub struct TokenParams {
    pub grant_type: Option<String>,
    pub code: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_verifier: Option<String>,
}

/// PKCE（S256）の検証。
fn pkce_matches(challenge: &str, verifier: &str) -> bool {
    use base64::Engine;
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest) == challenge
}

/// POST /oauth/token — 認可コードをアクセストークンに引き換える。
pub async fn token(
    State(state): State<AppState>,
    MastodonParams(params): MastodonParams<TokenParams>,
) -> Response {
    match params.grant_type.as_deref() {
        Some("authorization_code") => {}
        _ => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "unsupported_grant_type",
                "Only the authorization_code grant is supported.",
            )
        }
    }
    let (Some(client_id), Some(code), Some(redirect_uri)) =
        (params.client_id, params.code, params.redirect_uri)
    else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "client_id, code and redirect_uri are required.",
        );
    };
    let app = match oauth::find_app_by_client_id(&state.db, &client_id).await {
        Ok(Some(app)) => app,
        Ok(None) => {
            return oauth_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "Client authentication failed.",
            )
        }
        Err(e) => return internal(e).into_response(),
    };
    let secret_ok = params
        .client_secret
        .as_deref()
        .is_some_and(|s| sha256_hex(s) == app.client_secret_hash);
    if params.client_secret.is_some() && !secret_ok {
        return oauth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "Client authentication failed.",
        );
    }

    let consumed =
        match oauth::consume_authorization_code(&state.db, &sha256_hex(&code), app.id).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "The authorization code is invalid or expired.",
                )
            }
            Err(e) => return internal(e).into_response(),
        };
    if consumed.redirect_uri != redirect_uri {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "redirect_uri does not match.",
        );
    }
    // PKCE を使った認可ならベリファイアが必須。使っていないならクライアントシークレットが必須
    // （どちらも無いと、コードを盗み見ただけの第三者が交換できてしまう）。
    let pkce_ok = match (&consumed.code_challenge, &params.code_verifier) {
        (Some(challenge), Some(verifier)) => pkce_matches(challenge, verifier),
        (Some(_), None) => false,
        (None, _) => secret_ok,
    };
    if !pkce_ok {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "Client verification failed.",
        );
    }

    let (access_token, jti) = match state
        .local_auth
        .generate_app_token(consumed.user_id, &consumed.email)
    {
        Ok(t) => t,
        Err(e) => return internal(e).into_response(),
    };
    if let Err(e) =
        oauth::insert_app_token(&state.db, jti, consumed.user_id, &app.name, app.id, None).await
    {
        return internal(e).into_response();
    }
    Json(MastodonToken {
        access_token,
        token_type: "Bearer",
        scope: consumed.scopes,
        created_at: chrono::Utc::now().timestamp(),
    })
    .into_response()
}

#[derive(Deserialize)]
pub struct RevokeParams {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub token: Option<String>,
}

/// POST /oauth/revoke — クライアント自身が発行を受けたトークンを取り消す（ログアウト）。
/// RFC 7009 に従い、対象が無効・不明なトークンでも 200 を返す。
pub async fn revoke(
    State(state): State<AppState>,
    MastodonParams(params): MastodonParams<RevokeParams>,
) -> Response {
    let (Some(client_id), Some(token)) = (params.client_id, params.token) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "client_id and token are required.",
        );
    };
    let app = match oauth::find_app_by_client_id(&state.db, &client_id).await {
        Ok(Some(app))
            if params
                .client_secret
                .as_deref()
                .is_some_and(|s| sha256_hex(s) == app.client_secret_hash) =>
        {
            app
        }
        Ok(_) => {
            return oauth_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "Client authentication failed.",
            )
        }
        Err(e) => return internal(e).into_response(),
    };
    if let Ok(verified) = state.local_auth.verify_token_ignoring_exp(&token) {
        if let Err(e) = oauth::revoke_app_token(&state.db, verified.jti, app.id).await {
            return internal(e).into_response();
        }
    }
    Json(serde_json::json!({})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uri_rules() {
        assert!(is_valid_redirect_uri(OOB_REDIRECT_URI));
        assert!(is_valid_redirect_uri("https://elk.zone/api/seiran/oauth"));
        assert!(is_valid_redirect_uri("tusky://oauth"));
        assert!(is_valid_redirect_uri("http://localhost:8080/callback"));
        assert!(is_valid_redirect_uri("http://127.0.0.1/cb"));
        assert!(!is_valid_redirect_uri("http://example.com/cb"));
        assert!(!is_valid_redirect_uri("javascript:alert(1)"));
        assert!(!is_valid_redirect_uri("data:text/html,x"));
        assert!(!is_valid_redirect_uri("not a url"));
    }

    #[test]
    fn redirect_uris_accept_space_and_newline_separated() {
        assert_eq!(
            split_redirect_uris(vec!["a://x b://y\nc://z".to_owned()]),
            vec!["a://x", "b://y", "c://z"]
        );
    }

    #[test]
    fn code_and_state_are_appended_to_redirect_uri() {
        assert_eq!(
            redirect_with_code("https://app.example/cb?x=1", "abc", Some("s t")),
            "https://app.example/cb?x=1&code=abc&state=s+t"
        );
        assert_eq!(
            redirect_with_code("tusky://oauth", "abc", None),
            "tusky://oauth?code=abc"
        );
    }

    #[test]
    fn pkce_s256_matches_rfc7636_example() {
        // RFC 7636 Appendix B
        assert!(pkce_matches(
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        ));
        assert!(!pkce_matches(
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "wrong"
        ));
    }
}
