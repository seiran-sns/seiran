//! OAuth クライアントの識別と認証。AT Protocol の OAuth には事前のクライアント登録が無く、
//! `client_id` はクライアントメタデータ（JSON）の URL そのもの。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use seiran_common::replay_guard::ReplayGuard;

/// 取得したメタデータを使い回す時間。
const METADATA_CACHE_TTL: Duration = Duration::from_secs(300);
/// クライアント認証 JWT の `iat` として受け付ける古さ。
const ASSERTION_MAX_AGE_SECS: i64 = 300;
pub const CLIENT_ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";

#[derive(Debug, Clone)]
pub struct ClientError(pub &'static str);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationType {
    Web,
    Native,
}

/// 検証済みのクライアント。
#[derive(Debug, Clone)]
pub struct Client {
    pub client_id: String,
    pub client_name: Option<String>,
    pub client_uri: Option<String>,
    pub logo_uri: Option<String>,
    pub application_type: ApplicationType,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    /// 機密クライアント（`private_key_jwt`）の公開鍵。公開クライアントなら None。
    pub jwks: Option<jsonwebtoken::jwk::JwkSet>,
    /// `client_id` が `http://localhost` の開発用クライアント。
    pub loopback: bool,
}

impl Client {
    pub fn is_confidential(&self) -> bool {
        self.jwks.is_some()
    }

    /// 承認画面に出すホスト名（クライアント名・ロゴは自己申告なので、これを必ず並べる）。
    pub fn host(&self) -> String {
        url::Url::parse(&self.client_id)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// 要求された `redirect_uri` が登録済みのどれかと一致するか。ループバックの IP 宛ては、
    /// ポートだけ違っても一致とみなす（ネイティブアプリは起動のたびに空いているポートを使う。RFC 8252）。
    pub fn allows_redirect_uri(&self, requested: &str) -> bool {
        if self.redirect_uris.iter().any(|u| u == requested) {
            return true;
        }
        let Ok(req) = url::Url::parse(requested) else {
            return false;
        };
        if req.scheme() != "http" || !is_loopback_ip_host(&req) {
            return false;
        }
        self.redirect_uris.iter().any(|u| {
            url::Url::parse(u).is_ok_and(|reg| {
                reg.scheme() == "http"
                    && reg.host_str() == req.host_str()
                    && reg.path() == req.path()
                    && reg.query() == req.query()
            })
        })
    }
}

fn is_loopback_ip_host(u: &url::Url) -> bool {
    matches!(u.host_str(), Some("127.0.0.1") | Some("[::1]"))
}

#[derive(Deserialize)]
struct RawMetadata {
    client_id: Option<String>,
    client_name: Option<String>,
    client_uri: Option<String>,
    logo_uri: Option<String>,
    application_type: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
    scope: Option<String>,
    redirect_uris: Option<Vec<String>>,
    dpop_bound_access_tokens: Option<bool>,
    token_endpoint_auth_method: Option<String>,
    token_endpoint_auth_signing_alg: Option<String>,
    jwks: Option<jsonwebtoken::jwk::JwkSet>,
    jwks_uri: Option<String>,
}

fn cache() -> &'static Mutex<HashMap<String, (Instant, Client)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Client)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `client_id` からクライアントを解決し、メタデータを検証する。
pub async fn resolve(client_id: &str) -> Result<Client, ClientError> {
    if is_loopback_client_id(client_id) {
        return loopback_client(client_id);
    }
    if let Some((at, client)) = cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(client_id)
    {
        if at.elapsed() < METADATA_CACHE_TTL {
            return Ok(client.clone());
        }
    }

    let url = url::Url::parse(client_id).map_err(|_| ClientError("invalid client_id"))?;
    if url.scheme() != "https"
        || url.fragment().is_some()
        || url.username() != ""
        || url.password().is_some()
        || url.path().is_empty()
        || url.path() == "/"
    {
        return Err(ClientError(
            "client_id must be an https URL of the client metadata",
        ));
    }
    let value = seiran_common::net::fetch_json_validated(client_id)
        .await
        .map_err(|_| ClientError("failed to fetch client metadata"))?;
    let raw: RawMetadata =
        serde_json::from_value(value).map_err(|_| ClientError("invalid client metadata"))?;
    let client = validate_metadata(client_id, raw).await?;

    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() > 1000 {
        cache.retain(|_, (at, _)| at.elapsed() < METADATA_CACHE_TTL);
    }
    cache.insert(client_id.to_string(), (Instant::now(), client.clone()));
    Ok(client)
}

async fn validate_metadata(client_id: &str, raw: RawMetadata) -> Result<Client, ClientError> {
    if raw.client_id.as_deref() != Some(client_id) {
        return Err(ClientError("client_id does not match the metadata URL"));
    }
    if !raw
        .grant_types
        .as_ref()
        .is_some_and(|g| g.iter().any(|t| t == "authorization_code"))
    {
        return Err(ClientError("grant_types must include authorization_code"));
    }
    if !raw
        .response_types
        .as_ref()
        .is_some_and(|r| r.iter().any(|t| t == "code"))
    {
        return Err(ClientError("response_types must include code"));
    }
    if raw.dpop_bound_access_tokens != Some(true) {
        return Err(ClientError("dpop_bound_access_tokens must be true"));
    }
    let scopes = split_scope(raw.scope.as_deref().unwrap_or_default());
    if !scopes.iter().any(|s| s == "atproto") {
        return Err(ClientError("scope must include atproto"));
    }
    let application_type = match raw.application_type.as_deref() {
        None | Some("web") => ApplicationType::Web,
        Some("native") => ApplicationType::Native,
        Some(_) => return Err(ClientError("invalid application_type")),
    };
    let redirect_uris = raw.redirect_uris.unwrap_or_default();
    if redirect_uris.is_empty() {
        return Err(ClientError("redirect_uris must not be empty"));
    }
    let client_host = url::Url::parse(client_id)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    for uri in &redirect_uris {
        validate_redirect_uri(uri, &application_type, &client_host)?;
    }

    let jwks = match raw.token_endpoint_auth_method.as_deref() {
        None | Some("none") => None,
        Some("private_key_jwt") => {
            if raw.token_endpoint_auth_signing_alg.as_deref() != Some("ES256") {
                return Err(ClientError("token_endpoint_auth_signing_alg must be ES256"));
            }
            let set = match (raw.jwks, raw.jwks_uri) {
                (Some(set), None) => set,
                (None, Some(uri)) => {
                    let value = seiran_common::net::fetch_json_validated(&uri)
                        .await
                        .map_err(|_| ClientError("failed to fetch jwks_uri"))?;
                    serde_json::from_value(value).map_err(|_| ClientError("invalid jwks"))?
                }
                _ => return Err(ClientError("exactly one of jwks or jwks_uri is required")),
            };
            if set.keys.is_empty() {
                return Err(ClientError("jwks must not be empty"));
            }
            Some(set)
        }
        Some(_) => return Err(ClientError("unsupported token_endpoint_auth_method")),
    };

    Ok(Client {
        client_id: client_id.to_string(),
        client_name: raw.client_name.filter(|n| !n.trim().is_empty()),
        client_uri: raw.client_uri,
        logo_uri: raw.logo_uri.filter(|u| u.starts_with("https://")),
        application_type,
        redirect_uris,
        scopes,
        jwks,
        loopback: false,
    })
}

/// Web アプリは https だけ。ネイティブアプリは https・ループバック IP の http・
/// `client_id` のホスト名を逆順にしたカスタムスキーム（`com.example.app:/callback`）。
fn validate_redirect_uri(
    uri: &str,
    application_type: &ApplicationType,
    client_host: &str,
) -> Result<(), ClientError> {
    let parsed = url::Url::parse(uri).map_err(|_| ClientError("invalid redirect_uri"))?;
    if parsed.fragment().is_some() {
        return Err(ClientError("redirect_uri must not have a fragment"));
    }
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if *application_type == ApplicationType::Native && is_loopback_ip_host(&parsed) => {
            Ok(())
        }
        "http" => Err(ClientError("redirect_uri must use https")),
        scheme if *application_type == ApplicationType::Native => {
            let reversed: Vec<&str> = client_host.split('.').rev().collect();
            if scheme == reversed.join(".") && !uri[scheme.len() + 1..].starts_with("//") {
                Ok(())
            } else {
                Err(ClientError(
                    "custom scheme redirect_uri must be the reversed client_id host",
                ))
            }
        }
        _ => Err(ClientError("redirect_uri must use https")),
    }
}

fn is_loopback_client_id(client_id: &str) -> bool {
    client_id == "http://localhost"
        || client_id.starts_with("http://localhost?")
        || client_id.starts_with("http://localhost/?")
        || client_id == "http://localhost/"
}

/// 開発用の `http://localhost` クライアント。メタデータは取りに行かず、クエリの
/// `redirect_uri`（複数可）と `scope` から作る。公開クライアントで、戻り先はループバック IP だけ。
fn loopback_client(client_id: &str) -> Result<Client, ClientError> {
    let url = url::Url::parse(client_id).map_err(|_| ClientError("invalid client_id"))?;
    let mut redirect_uris = Vec::new();
    let mut scope = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "redirect_uri" => redirect_uris.push(v.into_owned()),
            "scope" => scope = Some(v.into_owned()),
            _ => {}
        }
    }
    if redirect_uris.is_empty() {
        redirect_uris = vec!["http://127.0.0.1/".to_string(), "http://[::1]/".to_string()];
    }
    for uri in &redirect_uris {
        let parsed = url::Url::parse(uri).map_err(|_| ClientError("invalid redirect_uri"))?;
        if parsed.scheme() != "http" || !is_loopback_ip_host(&parsed) {
            return Err(ClientError(
                "loopback client redirect_uri must be http://127.0.0.1 or http://[::1]",
            ));
        }
    }
    let scopes = split_scope(scope.as_deref().unwrap_or("atproto"));
    if !scopes.iter().any(|s| s == "atproto") {
        return Err(ClientError("scope must include atproto"));
    }
    Ok(Client {
        client_id: client_id.to_string(),
        client_name: None,
        client_uri: None,
        logo_uri: None,
        application_type: ApplicationType::Native,
        redirect_uris,
        scopes,
        jwks: None,
        loopback: true,
    })
}

pub fn split_scope(scope: &str) -> Vec<String> {
    scope.split_whitespace().map(str::to_string).collect()
}

#[derive(Deserialize)]
struct AssertionClaims {
    iss: String,
    sub: String,
    jti: Option<String>,
    iat: Option<i64>,
}

/// 機密クライアントの `client_assertion`（`private_key_jwt`）を検証し、使われた鍵の `kid` を返す。
pub async fn verify_client_assertion(
    client: &Client,
    assertion_type: Option<&str>,
    assertion: Option<&str>,
    issuer: &str,
    replay: &ReplayGuard,
) -> Result<String, ClientError> {
    let jwks = client
        .jwks
        .as_ref()
        .ok_or(ClientError("client is not confidential"))?;
    if assertion_type != Some(CLIENT_ASSERTION_TYPE) {
        return Err(ClientError("client_assertion_type is required"));
    }
    let assertion = assertion.ok_or(ClientError("client_assertion is required"))?;
    let header = jsonwebtoken::decode_header(assertion)
        .map_err(|_| ClientError("invalid client_assertion"))?;
    if header.alg != Algorithm::ES256 {
        return Err(ClientError("client_assertion must be ES256"));
    }
    let kid = header
        .kid
        .ok_or(ClientError("client_assertion must have kid"))?;
    let jwk = jwks
        .find(&kid)
        .ok_or(ClientError("unknown client_assertion kid"))?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| ClientError("invalid client key"))?;
    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_audience(&[issuer]);
    validation.set_issuer(&[client.client_id.as_str()]);
    validation.set_required_spec_claims(&["exp", "aud", "iss", "sub"]);
    let claims = jsonwebtoken::decode::<AssertionClaims>(assertion, &key, &validation)
        .map_err(|_| ClientError("invalid client_assertion"))?
        .claims;
    if claims.sub != client.client_id || claims.iss != client.client_id {
        return Err(ClientError("client_assertion sub must be client_id"));
    }
    let iat = claims
        .iat
        .ok_or(ClientError("client_assertion must have iat"))?;
    if chrono::Utc::now().timestamp() - iat > ASSERTION_MAX_AGE_SECS {
        return Err(ClientError("client_assertion is too old"));
    }
    let jti = claims
        .jti
        .ok_or(ClientError("client_assertion must have jti"))?;
    let first = replay
        .first_use(
            &format!("client-assertion:{}:{}", client.client_id, jti),
            Duration::from_secs(ASSERTION_MAX_AGE_SECS as u64 * 2),
        )
        .await
        .map_err(|_| ClientError("temporarily unavailable"))?;
    if !first {
        return Err(ClientError("client_assertion was already used"));
    }
    Ok(kid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_client_uses_query_and_defaults() {
        let c = loopback_client("http://localhost").unwrap();
        assert!(c.allows_redirect_uri("http://127.0.0.1/"));
        assert!(c.allows_redirect_uri("http://127.0.0.1:8123/"));
        assert!(!c.allows_redirect_uri("http://localhost/"));
        assert_eq!(c.scopes, vec!["atproto"]);

        let c = loopback_client(
            "http://localhost?redirect_uri=http%3A%2F%2F127.0.0.1%2Fcb&scope=atproto%20transition%3Ageneric",
        )
        .unwrap();
        assert!(c.allows_redirect_uri("http://127.0.0.1:5000/cb"));
        assert!(!c.allows_redirect_uri("http://127.0.0.1/other"));
        assert_eq!(c.scopes, vec!["atproto", "transition:generic"]);
        assert!(!c.is_confidential());

        assert!(
            loopback_client("http://localhost?redirect_uri=https%3A%2F%2Fevil.example%2F").is_err()
        );
    }

    #[test]
    fn redirect_uri_rules_depend_on_application_type() {
        let web = ApplicationType::Web;
        let native = ApplicationType::Native;
        assert!(
            validate_redirect_uri("https://app.example.com/cb", &web, "app.example.com").is_ok()
        );
        assert!(validate_redirect_uri("http://127.0.0.1/cb", &web, "app.example.com").is_err());
        assert!(validate_redirect_uri("http://127.0.0.1/cb", &native, "app.example.com").is_ok());
        assert!(validate_redirect_uri("com.example.app:/cb", &native, "app.example.com").is_ok());
        assert!(validate_redirect_uri("com.evil.app:/cb", &native, "app.example.com").is_err());
        assert!(validate_redirect_uri("com.example.app://cb", &native, "app.example.com").is_err());
        assert!(validate_redirect_uri("javascript:alert(1)", &web, "app.example.com").is_err());
    }

    #[tokio::test]
    async fn metadata_must_match_and_be_dpop_bound() {
        let base = || RawMetadata {
            client_id: Some("https://app.example.com/client-metadata.json".to_string()),
            client_name: Some("App".to_string()),
            client_uri: None,
            logo_uri: None,
            application_type: None,
            grant_types: Some(vec!["authorization_code".into(), "refresh_token".into()]),
            response_types: Some(vec!["code".into()]),
            scope: Some("atproto transition:generic".to_string()),
            redirect_uris: Some(vec!["https://app.example.com/cb".to_string()]),
            dpop_bound_access_tokens: Some(true),
            token_endpoint_auth_method: Some("none".to_string()),
            token_endpoint_auth_signing_alg: None,
            jwks: None,
            jwks_uri: None,
        };
        let id = "https://app.example.com/client-metadata.json";
        assert!(validate_metadata(id, base()).await.is_ok());
        assert!(validate_metadata("https://other.example/m.json", base())
            .await
            .is_err());
        let mut m = base();
        m.dpop_bound_access_tokens = None;
        assert!(validate_metadata(id, m).await.is_err());
        let mut m = base();
        m.scope = Some("transition:generic".to_string());
        assert!(validate_metadata(id, m).await.is_err());
        let mut m = base();
        m.token_endpoint_auth_method = Some("private_key_jwt".to_string());
        assert!(validate_metadata(id, m).await.is_err());
    }
}
