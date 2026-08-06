//! Trusted reverse-proxy headersからレート制限用クライアントIPを解決する。

use std::convert::Infallible;
use std::net::IpAddr;

use async_trait::async_trait;
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientIp(pub Option<String>);

impl ClientIp {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        if let Some(ip) = header_ip(headers, "cf-connecting-ip") {
            return Self(Some(ip));
        }
        if let Some(ip) = header_ip(headers, "x-real-ip") {
            return Self(Some(ip));
        }
        if let Some(value) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        {
            if let Some(ip) = parse_ip(value.split(',').next().unwrap_or_default()) {
                return Self(Some(ip));
            }
        }
        Self(None)
    }
}

fn header_ip(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_ip)
}

fn parse_ip(value: &str) -> Option<String> {
    value.trim().parse::<IpAddr>().ok().map(|ip| ip.to_string())
}

#[async_trait]
impl<S> FromRequestParts<S> for ClientIp
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::from_headers(&parts.headers))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn rejects_invalid_forwarded_address() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("not-an-ip"));
        assert_eq!(ClientIp::from_headers(&headers), ClientIp(None));
    }

    #[test]
    fn takes_first_forwarded_address() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.7, 10.0.0.1"),
        );
        assert_eq!(
            ClientIp::from_headers(&headers),
            ClientIp(Some("203.0.113.7".to_owned()))
        );
    }
}
