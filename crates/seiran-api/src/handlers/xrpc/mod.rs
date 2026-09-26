pub mod actor;
pub mod identity;
pub mod post_from_record;
pub mod proxy;
pub mod repo;
pub mod server;
pub mod sync;

use axum::http::HeaderMap;

use seiran_common::auth::local::VerifiedAtpAccess;

use crate::error::ApiError;
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

/// DID移行（PLC操作）・アカウント無効化のような権限の強い操作は、メインパスワードで
/// ログインしたセッションにだけ許す。アプリパスワードはサードパーティに渡すためのもので、
/// これを許すと渡した相手がDIDを乗っ取れる（公式PDSと同じ制限）。
pub(crate) fn require_privileged_session(verified: &VerifiedAtpAccess) -> Result<(), ApiError> {
    if verified.privileged {
        Ok(())
    } else {
        Err(ApiError::Forbidden("APP_PASSWORD_NOT_PERMITTED"))
    }
}
