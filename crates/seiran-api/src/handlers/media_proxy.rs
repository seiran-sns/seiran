use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    extract::Query,
    http::{header, HeaderValue, Response, StatusCode},
};
use reqwest::{redirect::Policy, Url};
use serde::Deserialize;

use crate::error::ApiError;

const MAX_MEDIA_BYTES: u64 = 25 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Deserialize)]
pub struct ProxyQuery {
    url: String,
}

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
                || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
                || (ip.octets()[0] == 192 && ip.octets()[1] == 0 && ip.octets()[2] == 0)
                || (ip.octets()[0] == 198 && (ip.octets()[1] == 18 || ip.octets()[1] == 19)))
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

async fn validate_url(raw: &str) -> Result<(Url, Vec<SocketAddr>), ApiError> {
    let url = Url::parse(raw).map_err(|_| ApiError::BadRequest("INVALID_PROXY_URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::BadRequest("INVALID_PROXY_URL".into()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ApiError::BadRequest("INVALID_PROXY_URL".into()))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| ApiError::BadRequest("INVALID_PROXY_URL".into()))?;
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| ApiError::BadGateway("MEDIA_PROXY_DNS_FAILED".into()))?
        .collect();
    for address in &addresses {
        if !is_public_ip(address.ip()) {
            return Err(ApiError::Forbidden("MEDIA_PROXY_PRIVATE_ADDRESS"));
        }
    }
    if addresses.is_empty() {
        return Err(ApiError::BadGateway("MEDIA_PROXY_DNS_FAILED".into()));
    }
    Ok((url, addresses))
}

/// 検証済みURLから本文を取得する（SSRF対策込み: private/loopback/link-local等のIPへの接続を拒否し、
/// リダイレクト先も毎回同じ検証を通す）。`/proxy` エンドポイントとリモート絵文字インポート
/// （`handlers::admin::remote_emojis`, #73）の両方から使う共通ロジック。
/// `accept_prefixes` に前方一致しない `Content-Type` は `MEDIA_PROXY_UNSUPPORTED_TYPE` として拒否する。
pub async fn fetch_validated(
    raw_url: &str,
    accept_prefixes: &[&str],
) -> Result<(Bytes, String), ApiError> {
    fetch_validated_with_accept(
        raw_url,
        accept_prefixes,
        "image/*,video/*,audio/*;q=0.9,*/*;q=0.1",
    )
    .await
}

/// `fetch_validated`と同じSSRF防御を使い、用途別のAcceptヘッダーで取得する。
pub async fn fetch_validated_with_accept(
    raw_url: &str,
    accept_prefixes: &[&str],
    accept_header: &str,
) -> Result<(Bytes, String), ApiError> {
    let (mut url, mut addresses) = validate_url(raw_url).await?;

    for redirect_count in 0..=MAX_REDIRECTS {
        let host = url
            .host_str()
            .ok_or_else(|| ApiError::BadRequest("INVALID_PROXY_URL".into()))?;
        // 検証したIPへ接続先を固定し、検証後のDNS rebindingを防ぐ。
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .user_agent("seiran-media-proxy/1.0")
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        let upstream = client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, accept_header)
            .send()
            .await
            .map_err(|_| ApiError::BadGateway("MEDIA_PROXY_FETCH_FAILED".into()))?;

        if upstream.status().is_redirection() {
            if redirect_count == MAX_REDIRECTS {
                return Err(ApiError::BadGateway(
                    "MEDIA_PROXY_TOO_MANY_REDIRECTS".into(),
                ));
            }
            let location = upstream
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| ApiError::BadGateway("MEDIA_PROXY_INVALID_REDIRECT".into()))?;
            let next = url
                .join(location)
                .map_err(|_| ApiError::BadGateway("MEDIA_PROXY_INVALID_REDIRECT".into()))?;
            (url, addresses) = validate_url(next.as_str()).await?;
            continue;
        }

        if !upstream.status().is_success() {
            return Err(ApiError::BadGateway("MEDIA_PROXY_UPSTREAM_ERROR".into()));
        }
        if upstream
            .content_length()
            .is_some_and(|size| size > MAX_MEDIA_BYTES)
        {
            return Err(ApiError::BadGateway("MEDIA_PROXY_TOO_LARGE".into()));
        }
        let content_type = upstream
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        if !accept_prefixes.iter().any(|p| content_type.starts_with(p)) {
            return Err(ApiError::BadGateway("MEDIA_PROXY_UNSUPPORTED_TYPE".into()));
        }
        let bytes = upstream
            .bytes()
            .await
            .map_err(|_| ApiError::BadGateway("MEDIA_PROXY_FETCH_FAILED".into()))?;
        if bytes.len() as u64 > MAX_MEDIA_BYTES {
            return Err(ApiError::BadGateway("MEDIA_PROXY_TOO_LARGE".into()));
        }
        return Ok((bytes, content_type));
    }
    unreachable!()
}

/// Misskey互換 GET /proxy?url=...
pub async fn proxy(Query(query): Query<ProxyQuery>) -> Result<Response<Body>, ApiError> {
    let (bytes, content_type) =
        fetch_validated(&query.url, &["image/", "video/", "audio/"]).await?;
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::is_public_ip;

    #[test]
    fn rejects_non_public_addresses() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
