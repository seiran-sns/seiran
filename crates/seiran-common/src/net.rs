//! SSRF対策込みの外部URLフェッチ（`/proxy`・リモート絵文字インポート・URLカードOGP取得で共有）。
//! private/loopback/link-local等のIPへの接続を拒否し、リダイレクト先も毎回同じ検証を通す。

use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use bytes::Bytes;
use reqwest::{redirect::Policy, Url};

const MAX_FETCH_BYTES: u64 = 25 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug)]
pub enum FetchError {
    InvalidUrl,
    DnsFailed,
    PrivateAddress,
    FetchFailed,
    TooManyRedirects,
    InvalidRedirect,
    UpstreamError,
    TooLarge,
    UnsupportedType,
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            FetchError::InvalidUrl => "INVALID_URL",
            FetchError::DnsFailed => "DNS_FAILED",
            FetchError::PrivateAddress => "PRIVATE_ADDRESS",
            FetchError::FetchFailed => "FETCH_FAILED",
            FetchError::TooManyRedirects => "TOO_MANY_REDIRECTS",
            FetchError::InvalidRedirect => "INVALID_REDIRECT",
            FetchError::UpstreamError => "UPSTREAM_ERROR",
            FetchError::TooLarge => "TOO_LARGE",
            FetchError::UnsupportedType => "UNSUPPORTED_TYPE",
        };
        f.write_str(s)
    }
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

async fn validate_url(raw: &str) -> Result<(Url, Vec<SocketAddr>), FetchError> {
    let url = Url::parse(raw).map_err(|_| FetchError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(FetchError::InvalidUrl);
    }
    let host = url.host_str().ok_or(FetchError::InvalidUrl)?;
    let port = url.port_or_known_default().ok_or(FetchError::InvalidUrl)?;
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| FetchError::DnsFailed)?
        .collect();
    for address in &addresses {
        if !is_public_ip(address.ip()) {
            return Err(FetchError::PrivateAddress);
        }
    }
    if addresses.is_empty() {
        return Err(FetchError::DnsFailed);
    }
    Ok((url, addresses))
}

/// 検証済みURLから本文を取得する（SSRF対策込み）。`accept_prefixes`に前方一致しない
/// `Content-Type`は`UnsupportedType`として拒否する。
pub async fn fetch_validated_with_accept(
    raw_url: &str,
    accept_prefixes: &[&str],
    accept_header: &str,
) -> Result<(Bytes, String), FetchError> {
    let (mut url, mut addresses) = validate_url(raw_url).await?;

    for redirect_count in 0..=MAX_REDIRECTS {
        let host = url.host_str().ok_or(FetchError::InvalidUrl)?;
        // 検証したIPへ接続先を固定し、検証後のDNS rebindingを防ぐ。
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .user_agent("seiran-fetch/1.0")
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|_| FetchError::FetchFailed)?;
        let upstream = client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, accept_header)
            .send()
            .await
            .map_err(|_| FetchError::FetchFailed)?;

        if upstream.status().is_redirection() {
            if redirect_count == MAX_REDIRECTS {
                return Err(FetchError::TooManyRedirects);
            }
            let location = upstream
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchError::InvalidRedirect)?;
            let next = url
                .join(location)
                .map_err(|_| FetchError::InvalidRedirect)?;
            (url, addresses) = validate_url(next.as_str()).await?;
            continue;
        }

        if !upstream.status().is_success() {
            return Err(FetchError::UpstreamError);
        }
        if upstream
            .content_length()
            .is_some_and(|size| size > MAX_FETCH_BYTES)
        {
            return Err(FetchError::TooLarge);
        }
        let header_content_type = upstream
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let bytes = upstream
            .bytes()
            .await
            .map_err(|_| FetchError::FetchFailed)?;
        if bytes.len() as u64 > MAX_FETCH_BYTES {
            return Err(FetchError::TooLarge);
        }
        // ヘッダーのContent-Typeがホワイトリストに一致しない場合（media.misskeyusercontent.com等が
        // application/octet-streamを返すケースがある）、アップロード機能と同じマジックバイト判定で救済を試みる。
        let content_type = if accept_prefixes
            .iter()
            .any(|p| header_content_type.starts_with(p))
        {
            header_content_type
        } else {
            crate::storage::media_probe::sniff_mime_type(&bytes, &header_content_type)
        };
        if !accept_prefixes.iter().any(|p| content_type.starts_with(p)) {
            return Err(FetchError::UnsupportedType);
        }
        return Ok((bytes, content_type));
    }
    unreachable!()
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
