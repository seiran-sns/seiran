//! AT Protocol の OAuth プロファイルに沿った認可サーバー兼リソースサーバー。
//! 流れ・有効期間・スコープは `docs/protocols.md` 3節「OAuth」。エンドポイントは
//! `handlers::atproto_oauth`、XRPC での検証は `handlers::xrpc::auth`。

pub mod client;
pub mod dpop;
pub mod token;

use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::AppState;

/// PAR で受けた要求（`request_uri`）の有効期間。
pub const REQUEST_TTL_SECS: i64 = 300;
/// 認可コードの有効期間。
pub const CODE_TTL_SECS: i64 = 60;
/// 公開クライアントのセッションの上限（承認から。リフレッシュしても延びない）。
pub const PUBLIC_SESSION_TTL_DAYS: i64 = 14;
/// 機密クライアントのセッションの上限（最後のリフレッシュから）。
pub const CONFIDENTIAL_SESSION_TTL_DAYS: i64 = 90;
pub const REQUEST_URI_PREFIX: &str = "urn:ietf:params:oauth:request_uri:";

/// 付与できるスコープ。`transition:chat.bsky` 等ほかのスコープは要求されても付与しない。
pub const SUPPORTED_SCOPES: &[&str] = &["atproto", "transition:generic", "transition:email"];
pub const SCOPE_GENERIC: &str = "transition:generic";
pub const SCOPE_EMAIL: &str = "transition:email";

pub const PATH_PAR: &str = "/oauth/atproto/par";
pub const PATH_AUTHORIZE: &str = "/oauth/atproto/authorize";
pub const PATH_TOKEN: &str = "/oauth/atproto/token";
pub const PATH_REVOKE: &str = "/oauth/atproto/revoke";

/// 認可サーバーの issuer。PDS と同じ `https://{local_domain}`（DID ドキュメントの PDS から
/// たどり着いたクライアントが、`issuer` と完全に一致するかを確かめる）。
pub fn issuer(state: &AppState) -> String {
    format!("https://{}", state.local_domain)
}

/// DPoP 証明の `htu` と比べる、このサーバーの外から見た URL。トンネル・リバースプロキシ越しでは
/// リクエストの Host やスキームが外と違うことがあるので、常に `local_domain` から組み立てる。
pub fn external_url(state: &AppState, path: &str) -> String {
    format!("{}{}", issuer(state), path)
}

pub fn nonce_source(state: &AppState) -> dpop::NonceSource {
    dpop::NonceSource::new(&state.secrets.jwt_secret_bytes())
}

/// 要求されたスコープのうち、クライアントが登録していて seiran が付与できるものだけを残す。
/// `atproto` が無ければ None。
pub fn grantable_scope(requested: &[String], client_scopes: &[String]) -> Option<Vec<String>> {
    if !requested.iter().any(|s| s == "atproto") {
        return None;
    }
    let mut granted: Vec<String> = Vec::new();
    for s in requested {
        if SUPPORTED_SCOPES.contains(&s.as_str())
            && client_scopes.iter().any(|c| c == s)
            && !granted.contains(s)
        {
            granted.push(s.clone());
        }
    }
    granted.iter().any(|s| s == "atproto").then_some(granted)
}

/// トークン系エンドポイント（PAR・token・revoke）の OAuth エラー（RFC 6749 5.2）。
pub struct OAuthError {
    pub status: StatusCode,
    pub error: &'static str,
    pub description: String,
}

impl OAuthError {
    pub fn invalid_request(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: "invalid_request",
            description: description.into(),
        }
    }
    pub fn invalid_client(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            error: "invalid_client",
            description: description.into(),
        }
    }
    pub fn invalid_grant(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: "invalid_grant",
            description: description.into(),
        }
    }
    pub fn invalid_dpop_proof(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: "invalid_dpop_proof",
            description: description.into(),
        }
    }
    pub fn use_dpop_nonce() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: "use_dpop_nonce",
            description: "Authorization server requires nonce in DPoP proof".to_string(),
        }
    }
    pub fn server_error(log: impl std::fmt::Display) -> Self {
        tracing::error!("[atproto-oauth] {}", log);
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: "server_error",
            description: "internal error".to_string(),
        }
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let mut res = (
            self.status,
            Json(serde_json::json!({
                "error": self.error,
                "error_description": self.description,
            })),
        )
            .into_response();
        res.headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
        res
    }
}

/// トークン系エンドポイントの DPoP 証明を検証し、`jti` の再利用を拒否する。鍵の拇印を返す。
pub async fn verify_endpoint_dpop(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
) -> Result<String, OAuthError> {
    let proof = single_header(headers, "dpop")
        .ok_or_else(|| OAuthError::invalid_dpop_proof("DPoP proof is required"))?;
    let verified = dpop::verify(
        proof,
        &nonce_source(state),
        "POST",
        &external_url(state, path),
        None,
    )
    .map_err(|e| match e {
        dpop::DpopError::UseNonce => OAuthError::use_dpop_nonce(),
        dpop::DpopError::Invalid(m) => OAuthError::invalid_dpop_proof(m),
    })?;
    check_dpop_jti(state, &verified)
        .await
        .map_err(|e| match e {
            JtiError::Reused => OAuthError::invalid_dpop_proof("DPoP proof was already used"),
            JtiError::Unavailable(m) => OAuthError::server_error(m),
        })?;
    Ok(verified.jkt)
}

pub enum JtiError {
    Reused,
    Unavailable(String),
}

pub async fn check_dpop_jti(state: &AppState, proof: &dpop::DpopProof) -> Result<(), JtiError> {
    let first = state
        .replay_guard
        .first_use(
            &format!("dpop:{}:{}", proof.jkt, proof.jti),
            Duration::from_secs(dpop::JTI_TTL_SECS),
        )
        .await
        .map_err(|e| JtiError::Unavailable(format!("DPoP jti の記録に失敗: {}", e)))?;
    if first {
        Ok(())
    } else {
        Err(JtiError::Reused)
    }
}

/// 同じ名前のヘッダーが2つ以上あれば None（DPoP 証明は1つだけ）。
pub fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?;
    if values.next().is_some() {
        return None;
    }
    first.to_str().ok()
}

/// DPoP を使う応答には、次に使う nonce を付けておく（クライアントは古い nonce で失敗してから
/// 取り直さずに済む）。
pub fn attach_nonce(state: &AppState, res: &mut Response) {
    if let Ok(v) = HeaderValue::from_str(&nonce_source(state).current()) {
        res.headers_mut().insert("dpop-nonce", v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn grants_only_supported_and_registered_scopes() {
        let client = v(&["atproto", "transition:generic", "transition:chat.bsky"]);
        assert_eq!(
            grantable_scope(
                &v(&["atproto", "transition:generic", "transition:chat.bsky"]),
                &client
            ),
            Some(v(&["atproto", "transition:generic"]))
        );
        // クライアントが登録していないスコープは付与しない
        assert_eq!(
            grantable_scope(&v(&["atproto", "transition:email"]), &client),
            Some(v(&["atproto"]))
        );
        assert_eq!(grantable_scope(&v(&["transition:generic"]), &client), None);
    }
}
