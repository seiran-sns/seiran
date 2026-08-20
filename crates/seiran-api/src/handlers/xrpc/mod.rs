pub mod actor;
pub mod proxy;
pub mod repo;
pub mod server;
pub mod sync;

use axum::http::HeaderMap;

use crate::AppState;

/// このPDSのサービスDID（`did:web:{local_domain}`）。ATPセッションJWTの `aud` として使う。
pub(crate) fn service_did(state: &AppState) -> String {
    format!("did:web:{}", state.local_domain)
}

/// `Authorization: Bearer <token>` からトークン部分だけを取り出す。
pub(crate) fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
}
