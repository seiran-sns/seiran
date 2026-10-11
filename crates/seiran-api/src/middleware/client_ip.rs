//! レート制限・IPブロックに使うクライアントIPを決める。
//!
//! TCP の接続元から出発し、`X-Forwarded-For` を右（自分に近い側）から左へたどって、
//! 信頼するプロキシ（`TRUSTED_PROXIES`）でない最初のアドレスをクライアントとする。
//! XFF の各段は、そのリクエストを受け取ったプロキシが「自分に接続してきた相手」を末尾に
//! 追記したものなので、信頼するプロキシが書いた部分（右側）だけが信用できる。それより
//! 左はクライアントが自由に書けるので読まない。
//!
//! - 接続元が信頼するプロキシでなければ、ヘッダーは一切見ず接続元をクライアントとする
//!   （直接つないできた相手は XFF を好きに偽装できる）。
//! - `X-Real-IP`・`CF-Connecting-IP` は見ない。どの段で付いたものかを確かめる手段がない。
//!   Cloudflare のエッジも XFF の末尾に接続元を追記するので、XFF だけで足りる。
//! - たどり切っても信頼するプロキシしかいない（自分たちのインフラ内から来た）リクエストは
//!   クライアント不明（`None`、IP単位の制限の対象外）にする。
//!
//! `TRUSTED_PROXIES` はカンマ区切りの CIDR。既定はループバックとプライベートアドレス
//! （Docker のネットワーク内の nginx・cloudflared がここに入る）。Cloudflare のプロキシ
//! （tunnel ではなく DNS のプロキシ）を通す構成では、Cloudflare の IP 範囲を加える。

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;

use async_trait::async_trait;
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{request::Parts, HeaderMap},
};
use ipnet::IpNet;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientIp(pub Option<String>);

const DEFAULT_TRUSTED_PROXIES: &[&str] = &[
    "127.0.0.0/8",
    "::1/128",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "fc00::/7",
];

static TRUSTED_PROXIES: LazyLock<TrustedProxies> = LazyLock::new(TrustedProxies::from_env);

/// 信頼するリバースプロキシのアドレス範囲。
#[derive(Debug, Clone)]
pub struct TrustedProxies(Vec<IpNet>);

impl TrustedProxies {
    fn from_env() -> Self {
        match std::env::var("TRUSTED_PROXIES") {
            Ok(value) if !value.trim().is_empty() => Self::parse(&value),
            _ => Self::parse(&DEFAULT_TRUSTED_PROXIES.join(",")),
        }
    }

    /// カンマ区切りの CIDR（プレフィックス長を省いた単一アドレスも可）を読む。
    /// 読めない項目はログに出して無視する。
    pub fn parse(value: &str) -> Self {
        let nets = value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| {
                let parsed = s
                    .parse::<IpNet>()
                    .or_else(|_| s.parse::<IpAddr>().map(IpNet::from));
                if parsed.is_err() {
                    tracing::error!("[client_ip] TRUSTED_PROXIES の項目を読めません: {}", s);
                }
                parsed.ok()
            })
            .collect();
        Self(nets)
    }

    fn contains(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|net| net.contains(&ip))
    }
}

impl ClientIp {
    /// `peer` は TCP の接続元。分からない（テストで `ConnectInfo` が無い等）ときはクライアント不明。
    pub fn resolve(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &TrustedProxies) -> Self {
        let Some(peer) = peer.map(|ip| ip.to_canonical()) else {
            return Self(None);
        };
        if !trusted.contains(peer) {
            return Self(Some(peer.to_string()));
        }
        // 複数行に分かれて届いた XFF は、届いた順につなげると1行のときと同じ並びになる。
        let chain: Vec<&str> = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .collect();
        for hop in chain.iter().rev() {
            let Some(ip) = parse_hop(hop) else {
                // 信頼するプロキシは読めるアドレスしか書かないので、ここに来るのはプロキシが
                // XFF を追記せずに転送した設定ミス。どこから来たか分からないものとして扱う。
                tracing::warn!("[client_ip] X-Forwarded-For の項目を読めません: {:?}", hop);
                return Self(None);
            };
            if !trusted.contains(ip) {
                return Self(Some(ip.to_string()));
            }
        }
        Self(None)
    }
}

/// XFF の1項目を読む。ポート付き（`1.2.3.4:5678`・`[::1]:80`）も受け付ける。
fn parse_hop(hop: &str) -> Option<IpAddr> {
    let hop = hop.trim();
    hop.parse::<IpAddr>()
        .or_else(|_| hop.parse::<SocketAddr>().map(|addr| addr.ip()))
        .ok()
        .map(|ip| ip.to_canonical())
}

#[async_trait]
impl<S> FromRequestParts<S> for ClientIp
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());
        Ok(Self::resolve(peer, &parts.headers, &TRUSTED_PROXIES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn defaults() -> TrustedProxies {
        TrustedProxies::parse(&DEFAULT_TRUSTED_PROXIES.join(","))
    }

    fn xff(values: &[&'static str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append("x-forwarded-for", HeaderValue::from_static(value));
        }
        headers
    }

    fn resolve(peer: &str, headers: &HeaderMap) -> Option<String> {
        ClientIp::resolve(Some(peer.parse().unwrap()), headers, &defaults()).0
    }

    #[test]
    fn untrusted_peer_ignores_headers() {
        let mut headers = xff(&["198.51.100.9"]);
        headers.insert("cf-connecting-ip", HeaderValue::from_static("198.51.100.8"));
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.7"));
        assert_eq!(
            resolve("203.0.113.5", &headers),
            Some("203.0.113.5".to_owned())
        );
    }

    #[test]
    fn takes_rightmost_untrusted_hop_behind_proxies() {
        // 偽装した値, Cloudflare が追記した接続元, cloudflared, LB の nginx
        let headers = xff(&["198.51.100.66, 203.0.113.7, 172.18.0.5, 172.18.0.3"]);
        assert_eq!(
            resolve("127.0.0.1", &headers),
            Some("203.0.113.7".to_owned())
        );
    }

    #[test]
    fn joins_multiple_header_lines_in_order() {
        let headers = xff(&["198.51.100.66", "203.0.113.7, 10.0.0.2"]);
        assert_eq!(
            resolve("10.0.0.3", &headers),
            Some("203.0.113.7".to_owned())
        );
    }

    #[test]
    fn ignores_other_client_ip_headers_behind_proxies() {
        let mut headers = xff(&["203.0.113.7"]);
        headers.insert("cf-connecting-ip", HeaderValue::from_static("198.51.100.8"));
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.7"));
        assert_eq!(
            resolve("10.0.0.3", &headers),
            Some("203.0.113.7".to_owned())
        );
    }

    #[test]
    fn request_from_inside_trusted_network_has_no_client_ip() {
        assert_eq!(resolve("127.0.0.1", &HeaderMap::new()), None);
        assert_eq!(resolve("10.0.0.3", &xff(&["192.168.1.20"])), None);
    }

    #[test]
    fn unreadable_hop_next_to_trusted_proxy_gives_no_client_ip() {
        assert_eq!(resolve("10.0.0.3", &xff(&["203.0.113.7, unknown"])), None);
    }

    #[test]
    fn unreadable_hop_left_of_client_is_never_read() {
        assert_eq!(
            resolve("10.0.0.3", &xff(&["not-an-ip, 203.0.113.7"])),
            Some("203.0.113.7".to_owned())
        );
    }

    #[test]
    fn normalizes_ipv4_mapped_ipv6_and_ports() {
        assert_eq!(
            resolve(
                "::ffff:127.0.0.1",
                &xff(&["[2001:db8::1]:443, ::ffff:10.0.0.2"])
            ),
            Some("2001:db8::1".to_owned())
        );
        assert_eq!(
            resolve("::ffff:203.0.113.9", &HeaderMap::new()),
            Some("203.0.113.9".to_owned())
        );
        assert_eq!(
            resolve("127.0.0.1", &xff(&["203.0.113.7:51234"])),
            Some("203.0.113.7".to_owned())
        );
    }

    #[test]
    fn unknown_peer_has_no_client_ip() {
        assert_eq!(
            ClientIp::resolve(None, &xff(&["203.0.113.7"]), &defaults()),
            ClientIp(None)
        );
    }

    #[test]
    fn custom_trusted_proxies_accept_single_addresses() {
        let trusted = TrustedProxies::parse("192.0.2.10, 198.51.100.0/24, bogus");
        let headers = xff(&["203.0.113.7, 198.51.100.4"]);
        assert_eq!(
            ClientIp::resolve(Some("192.0.2.10".parse().unwrap()), &headers, &trusted).0,
            Some("203.0.113.7".to_owned())
        );
        // 既定のプライベートアドレスは信頼しなくなる
        assert_eq!(
            ClientIp::resolve(Some("10.0.0.3".parse().unwrap()), &headers, &trusted).0,
            Some("10.0.0.3".to_owned())
        );
    }
}
