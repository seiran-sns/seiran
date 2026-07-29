//! Fediverseリレー参加機能（#140）の管理者API。
//!
//! リレー本体は `actors` テーブルには登録しない（Mastodon本家のリレー実装と同様、
//! 管理者が入力した1つの inbox URL を Follow の object と実配送先の両方に使う）。

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use url::Url;

use seiran_common::repository::{Relay, RelayError};
use seiran_common::{job_priority, Job};

use crate::error::ApiError;
use crate::middleware::require_admin;
use crate::AppState;

// ─── レスポンス DTO ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RelayResponse {
    /// Snowflake ID は JavaScript の安全整数範囲を超えるため文字列で返す。
    pub id: String,
    pub inbox_url: String,
    pub status: String,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Relay> for RelayResponse {
    fn from(r: Relay) -> Self {
        Self {
            id: r.id.to_string(),
            inbox_url: r.inbox_url,
            status: r.status.as_str().to_string(),
            last_error: r.last_error,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

// ─── リクエスト DTO ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateRelayRequest {
    pub inbox_url: String,
}

// ─── エラー変換 ───────────────────────────────────────────────────────────

fn relay_err(e: RelayError) -> ApiError {
    match e {
        RelayError::DuplicateInboxUrl => ApiError::Conflict("DUPLICATE_INBOX_URL"),
        RelayError::Db(e) => ApiError::Internal(e.to_string()),
    }
}

/// `inbox_url` が HTTPS かつ userinfo（`user:pass@host`）を含まないことを検証する。
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 240
                || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1])))
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(mapped));
            }
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// HTTPS・userinfo無しに加え、登録時のDNS解決結果が公開IPだけであることを検証する。
async fn validate_inbox_url(raw: &str) -> Result<(), ApiError> {
    let url = Url::parse(raw).map_err(|_| ApiError::BadRequest("INVALID_INBOX_URL".into()))?;
    if url.scheme() != "https" {
        return Err(ApiError::BadRequest("INBOX_URL_MUST_BE_HTTPS".into()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ApiError::BadRequest("INVALID_INBOX_URL".into()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::BadRequest("INVALID_INBOX_URL".into()))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| ApiError::BadRequest("INBOX_DNS_FAILED".into()))?
        .collect();
    if addresses.is_empty() {
        return Err(ApiError::BadRequest("INBOX_DNS_FAILED".into()));
    }
    if addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(ApiError::BadRequest("INBOX_PRIVATE_ADDRESS".into()));
    }
    Ok(())
}

// ─── ハンドラ ─────────────────────────────────────────────────────────────

/// GET /api/admin/relays
pub async fn list_relays(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<RelayResponse>>, ApiError> {
    require_admin(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
    let relays = state.relays.list_all().await.map_err(relay_err)?;
    Ok(Json(relays.into_iter().map(Into::into).collect()))
}

/// POST /api/admin/relays
pub async fn create_relay(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateRelayRequest>,
) -> Result<Json<RelayResponse>, ApiError> {
    require_admin(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;
    validate_inbox_url(&req.inbox_url).await?;

    let relay = state
        .relays
        .insert(&req.inbox_url, &state.local_domain)
        .await
        .map_err(relay_err)?;

    state
        .job_queue
        .enqueue(
            Job::RelayFollowSync {
                relay_id: relay.id,
                want_follow: true,
            },
            job_priority::HIGH,
        )
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(relay.into()))
}

/// DELETE /api/admin/relays/:id
pub async fn delete_relay(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    require_admin(
        &headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await?;

    state
        .relays
        .find_by_id(id)
        .await
        .map_err(relay_err)?
        .ok_or(ApiError::NotFound("RELAY_NOT_FOUND"))?;

    state
        .job_queue
        .enqueue(
            Job::RelayFollowSync {
                relay_id: id,
                want_follow: false,
            },
            job_priority::HIGH,
        )
        .await
        .map_err(ApiError::Internal)?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::{validate_inbox_url, RelayResponse};
    use chrono::Utc;
    use seiran_common::repository::{Relay, RelayStatus};

    #[tokio::test]
    async fn accepts_public_https_inbox_url() {
        assert!(validate_inbox_url("https://8.8.8.8/inbox").await.is_ok());
    }

    #[tokio::test]
    async fn rejects_non_https_userinfo_and_private_ip() {
        assert!(validate_inbox_url("http://8.8.8.8/inbox").await.is_err());
        assert!(validate_inbox_url("https://user:pass@8.8.8.8/inbox")
            .await
            .is_err());
        assert!(validate_inbox_url("https://127.0.0.1/inbox").await.is_err());
        assert!(validate_inbox_url("not a url").await.is_err());
    }

    #[test]
    fn serializes_snowflake_id_without_javascript_rounding() {
        let now = Utc::now();
        let response = RelayResponse::from(Relay {
            id: 117_001_761_839_251_466,
            inbox_url: "https://relay.example/inbox".into(),
            status: RelayStatus::Pending,
            follow_activity_id: "https://example.com/activities/follow/relay-1".into(),
            last_error: None,
            created_at: now,
            updated_at: now,
        });

        let json = serde_json::to_value(response).expect("relay response must serialize");
        assert_eq!(json["id"], "117001761839251466");
    }
}
