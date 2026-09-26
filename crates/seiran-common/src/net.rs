//! SSRF対策込みの外部URLフェッチ（`/proxy`・リモート絵文字インポート・URLカードOGP取得で共有）。
//! private/loopback/link-local等のIPへの接続を拒否し、リダイレクト先も毎回同じ検証を通す。
//!
//! 連合（AP/ATP）用の共有`reqwest::Client`向けには、同じ公開IP判定を使うDNSリゾルバと
//! リダイレクトポリシー（`federation_client_builder`）、およびIPリテラルURLの事前検査
//! （`ensure_public_url`）を提供する。受信Activityの署名`keyId`・フォロー対象URL・
//! `ap/show`の`uri`など、外部から指定されたURLへサーバーが接続する経路はすべてこれを通す。

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::OnceLock,
    time::Duration,
};

use bytes::{Bytes, BytesMut};
use reqwest::{redirect::Policy, Url};

const MAX_FETCH_BYTES: u64 = 25 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
/// 連合用クライアントのリダイレクト上限（reqwestの既定値10に合わせる）。
const MAX_FEDERATION_REDIRECTS: usize = 10;

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

impl std::error::Error for FetchError {}

/// 内部ネットワーク宛の外向き通信を許可するか（`SEIRAN_ALLOW_PRIVATE_NETWORK=true`）。
/// E2Eのスタブサーバー（127.0.0.1）への連合など、ローカル検証専用のエスケープハッチ。
/// 本番・開発機では設定しないこと。プロセス起動時に一度だけ読む。
pub fn private_network_allowed() -> bool {
    static ALLOWED: OnceLock<bool> = OnceLock::new();
    *ALLOWED.get_or_init(|| {
        std::env::var("SEIRAN_ALLOW_PRIVATE_NETWORK").is_ok_and(|v| v == "true" || v == "1")
    })
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b == 18 || b == 19)))
}

/// IPv6アドレスに埋め込まれたIPv4（IPv4-mapped `::ffff:0:0/96`・IPv4互換 `::/96`・
/// NAT64 `64:ff9b::/96`・6to4 `2002::/16`）を取り出す。これらを素通しすると
/// `::ffff:127.0.0.1`や`64:ff9b::a9fe:a9fe`（169.254.169.254）経由で内部IPv4へ到達できる。
fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return Some(v4);
    }
    let seg = ip.segments();
    let tail = Ipv4Addr::new(
        (seg[6] >> 8) as u8,
        seg[6] as u8,
        (seg[7] >> 8) as u8,
        seg[7] as u8,
    );
    if seg[..6] == [0, 0, 0, 0, 0, 0] && !ip.is_loopback() && !ip.is_unspecified() {
        return Some(tail);
    }
    if seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return Some(tail);
    }
    if seg[0] == 0x2002 {
        return Some(Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            seg[1] as u8,
            (seg[2] >> 8) as u8,
            seg[2] as u8,
        ));
    }
    None
}

/// 外部から指定されたURLの接続先として許可できる公開IPか。`admin/relays`の登録時検証とも共有する。
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = embedded_ipv4(ip) {
                return is_public_ipv4(v4);
            }
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
                || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8))
        }
    }
}

/// URLのホストがIPリテラルなら公開IPか検査する（ドメイン名は`PublicOnlyResolver`が解決時に検査する）。
fn url_host_is_allowed(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => is_public_ipv4(ip),
        Some(url::Host::Ipv6(ip)) => is_public_ip(IpAddr::V6(ip)),
        Some(url::Host::Domain(_)) => true,
        None => false,
    }
}

/// 連合用クライアントで送る前に、外部から指定されたURLを検査する。reqwestはIPリテラルの
/// ホストをDNSリゾルバに渡さないため、`PublicOnlyResolver`だけでは`http://127.0.0.1/`等を
/// 止められない。スキームはhttp/httpsのみ許可する。
pub fn ensure_public_url(raw: &str) -> Result<(), FetchError> {
    let url = Url::parse(raw).map_err(|_| FetchError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(FetchError::InvalidUrl);
    }
    if private_network_allowed() || url_host_is_allowed(&url) {
        Ok(())
    } else {
        Err(FetchError::PrivateAddress)
    }
}

/// 解決結果に非公開IPが1つでも含まれるホスト名を拒否するDNSリゾルバ（`validate_url`と同じ基準）。
struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addresses.is_empty() || addresses.iter().any(|a| !is_public_ip(a.ip())) {
                return Err(Box::new(FetchError::PrivateAddress)
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// 連合（AP/ATP）用の共有クライアントのビルダー。`PublicOnlyResolver`とリダイレクト先の
/// IPリテラル検査を組み込む（`private_network_allowed()`のときは素のビルダー）。
pub fn federation_client_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    if private_network_allowed() {
        return builder;
    }
    builder
        .dns_resolver(std::sync::Arc::new(PublicOnlyResolver))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_FEDERATION_REDIRECTS {
                attempt.error(FetchError::TooManyRedirects)
            } else if !matches!(attempt.url().scheme(), "http" | "https")
                || !url_host_is_allowed(attempt.url())
            {
                attempt.error(FetchError::PrivateAddress)
            } else {
                attempt.follow()
            }
        }))
}

pub(crate) async fn validate_url(raw: &str) -> Result<(Url, Vec<SocketAddr>), FetchError> {
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

/// 本文を`MAX_FETCH_BYTES`まで読み込む。`Content-Length`が無い（chunked）応答でも、
/// 上限を超えた時点で打ち切る（全量をメモリに読み込んでから判定するとメモリ枯渇攻撃を許す）。
async fn read_body_limited(mut upstream: reqwest::Response) -> Result<Bytes, FetchError> {
    if upstream
        .content_length()
        .is_some_and(|size| size > MAX_FETCH_BYTES)
    {
        return Err(FetchError::TooLarge);
    }
    let mut buf = BytesMut::new();
    while let Some(chunk) = upstream
        .chunk()
        .await
        .map_err(|_| FetchError::FetchFailed)?
    {
        if (buf.len() + chunk.len()) as u64 > MAX_FETCH_BYTES {
            return Err(FetchError::TooLarge);
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}

/// 検証済みURLへ、リダイレクト先も毎回`validate_url`で検証し直しながらGETする。
/// 戻り値は（本文, ヘッダーのContent-Type）。`fetch_validated_with_accept`・
/// `fetch_json_validated`共通の取得ループ。
async fn fetch_following_validated_redirects(
    raw_url: &str,
    accept_header: &str,
    timeout: Duration,
) -> Result<(Bytes, String), FetchError> {
    let (mut url, mut addresses) = validate_url(raw_url).await?;

    for redirect_count in 0..=MAX_REDIRECTS {
        let host = url.host_str().ok_or(FetchError::InvalidUrl)?;
        // 検証したIPへ接続先を固定し、検証後のDNS rebindingを防ぐ。
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
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
        let header_content_type = upstream
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let bytes = read_body_limited(upstream).await?;
        return Ok((bytes, header_content_type));
    }
    unreachable!()
}

/// 検証済みURLから本文を取得する（SSRF対策込み）。`accept_prefixes`に前方一致しない
/// `Content-Type`は`UnsupportedType`として拒否する。
pub async fn fetch_validated_with_accept(
    raw_url: &str,
    accept_prefixes: &[&str],
    accept_header: &str,
) -> Result<(Bytes, String), FetchError> {
    let (bytes, header_content_type) =
        fetch_following_validated_redirects(raw_url, accept_header, Duration::from_secs(20))
            .await?;
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
    Ok((bytes, content_type))
}

/// SSRF対策込みでJSON文書を取得する（DIDドキュメント解決専用、[SEC-3]）。
/// `did:plc:`はPLCディレクトリ、`did:web:`は対象ドメイン自身の`.well-known/did.json`から
/// 取得するが、いずれもDID主体（＝リクエスト送信者が名乗るだけで取得できる相手）が内容を
/// 完全に制御できるため、`serviceEndpoint`はもちろんドキュメント取得自体のURLも
/// `fetch_validated_with_accept`と同じprivate/loopback/link-local等IP拒否・
/// DNS rebinding対策（検証済みIPへの接続固定）・リダイレクト先の再検証を経由させる。
/// メディア取得と異なりContent-Typeホワイトリストは適用しない（DID文書はJSONそのものを
/// パースできるかで妥当性を判断すれば十分なため）。
pub async fn fetch_json_validated(raw_url: &str) -> Result<serde_json::Value, FetchError> {
    let (bytes, _) = fetch_following_validated_redirects(
        raw_url,
        "application/json, application/did+json",
        Duration::from_secs(10),
    )
    .await?;
    serde_json::from_slice(&bytes).map_err(|_| FetchError::UpstreamError)
}

/// 本文中の生URL（`https?://\S+`、末尾の `)`/`]`/`.`/`,` 等の区切り記号は含めない）を
/// 出現順に検出し、重複を除いて返す（上限5件、Fediの本文URLカード抽出と同じ上限に揃える）。
/// Bsky embed選択（#227）の候補URL算出、および選択IDのバリデーションで使う。
pub fn extract_body_urls(text: &str) -> Vec<String> {
    const MAX_URLS: usize = 5;
    static URL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = URL_RE.get_or_init(|| regex::Regex::new(r#"https?://[^\s<>()\[\]]+"#).unwrap());

    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for m in re.find_iter(text) {
        let url = m.as_str();
        if seen.insert(url.to_string()) {
            result.push(url.to_string());
            if result.len() >= MAX_URLS {
                break;
            }
        }
    }
    result
}

/// ページのOGPメタデータ（`og:title`/`og:description`/`og:image`）＋
/// oEmbed discoveryで見つかった埋め込みプレーヤー情報。
pub struct OgpData {
    pub title: String,
    pub description: String,
    pub thumbnail_url: Option<String>,
    /// oEmbed discoveryで見つかったiframe src（ホワイトリスト判定前の生値）。
    /// `net.rs`はDBに依存しないためここではフィルタしない。呼び出し元が
    /// `oembed_whitelist::OembedWhitelist::is_allowed`で判定してから
    /// `post_link_cards.embed_src`へ保存すること。
    pub embed_src: Option<String>,
    /// oEmbedレスポンスの`type`（"video"/"rich"等）。
    pub embed_type: Option<String>,
}

/// HTTPヘッダーの`Content-Type`（`charset`パラメータ）と`<meta charset>`/
/// `<meta http-equiv="Content-Type" content="...">`から実際の文字コードを検出し、UTF-8文字列に
/// デコードする。日本語圏のサイトはEUC-JP/Shift_JISを使うことが少なくなく、常にUTF-8として
/// デコードすると本文が文字化けする（例: 楽天市場の商品ページは`charset=EUC-JP`）。
/// 優先順位はHTML5仕様のcharset検出に準じ、HTTPヘッダー→HTML内のmetaタグ→UTF-8フォールバック。
pub fn decode_html_body(bytes: &[u8], header_content_type: &str) -> String {
    let encoding = extract_charset_param(header_content_type)
        .or_else(|| extract_meta_charset(bytes))
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// `Content-Type: text/html; charset=EUC-JP`のような文字列から`charset`パラメータの値を取り出す。
fn extract_charset_param(content_type: &str) -> Option<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"(?i)charset=["']?([^;"'\s]+)"#).unwrap());
    re.captures(content_type)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim_end_matches('"').to_string())
}

/// HTML先頭部分（HTML5仕様のprescanに合わせ1024バイトまで）から`<meta charset="...">`または
/// `<meta http-equiv="Content-Type" content="text/html; charset=...">`のcharsetを検出する。
/// `<meta>`タグ自体はASCII文字のみで構成されるため、実際のエンコーディングが何であっても
/// `from_utf8_lossy`によるここでの走査でタグの構造が壊れることはない。
fn extract_meta_charset(bytes: &[u8]) -> Option<String> {
    static TAG_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static CHARSET_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let tag_re = TAG_RE.get_or_init(|| regex::Regex::new(r#"(?i)<meta\b[^>]*>"#).unwrap());
    let charset_re =
        CHARSET_RE.get_or_init(|| regex::Regex::new(r#"(?i)charset=["']?([^;"'\s>]+)"#).unwrap());

    let prefix_len = bytes.len().min(1024);
    let prefix = String::from_utf8_lossy(&bytes[..prefix_len]);
    tag_re
        .find_iter(&prefix)
        .find_map(|tag| charset_re.captures(tag.as_str()))
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

/// `<meta property="..." content="...">`（属性順序は問わない）から`content`を抽出する。
/// HTML5準拠の厳密なパースはせず、OGPメタタグの一般的な形だけを対象にした簡易実装。
fn extract_og_content(html: &str, property: &str) -> Option<String> {
    let escaped = regex::escape(property);
    let patterns = [
        format!(r#"<meta[^>]*?property=["']{escaped}["'][^>]*?content=["']([^"']*)["']"#),
        format!(r#"<meta[^>]*?content=["']([^"']*)["'][^>]*?property=["']{escaped}["']"#),
    ];
    for pattern in patterns {
        if let Ok(re) = regex::Regex::new(&pattern) {
            if let Some(cap) = re.captures(html) {
                let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                return Some(html_escape::decode_html_entities(raw).into_owned());
            }
        }
    }
    None
}

/// URLのOGPメタデータ + oEmbed埋め込み情報を取得する（SSRF対策込み、
/// `fetch_validated_with_accept`経由）。1回目のページ取得でog:*メタタグと
/// `<link rel="alternate" type=".../json+oembed">`を同時に抽出し、
/// oEmbedリンクタグが見つかった場合のみ2回目のJSON取得を行う。
///
/// `fixed_oembed_endpoint`が`Some`の場合、HTML内のdiscoveryタグ探索はスキップし、
/// 常にこのエンドポイントへ`?url=<url>&format=json`付きで直接oEmbedを取得する
/// （Vimeo等、oEmbed自体は提供するがHTMLにdiscoveryタグを載せていないサイト向けの
/// 管理者設定による救済、`oembed_whitelist::OembedWhitelist::fixed_endpoint_for`が
/// 解決する）。
///
/// フェッチ自体が失敗した場合は`Err`（呼び出し元がリトライ要否を判断できるよう
/// `FetchError`をそのまま返す）。フェッチはできたがog:title・oEmbedのいずれも
/// 見つからない場合は`Ok(None)`（リトライしても無駄）。どちらか一方でも見つかれば
/// `Ok(Some(..))`（titleが無ければ空文字列のまま）。`Job::OgpFetch`・
/// `Job::LinkCardEmbedResolve`・Bsky embed選択のURLカード生成から共有する。
pub async fn fetch_ogp(
    url: &str,
    fixed_oembed_endpoint: Option<&str>,
) -> Result<Option<OgpData>, FetchError> {
    let (bytes, content_type) = fetch_validated_with_accept(
        url,
        &["text/html", "application/xhtml+xml"],
        "text/html,application/xhtml+xml;q=0.9",
    )
    .await?;

    let html = decode_html_body(&bytes, &content_type);
    let base = Url::parse(url).ok();

    let title = extract_og_content(&html, "og:title");
    let description = extract_og_content(&html, "og:description").unwrap_or_default();
    let thumbnail_url = extract_og_content(&html, "og:image").and_then(|raw| {
        // 相対URLで書かれているサイトもあるため、対象ページのURLを基点に絶対URL化する。
        base.as_ref()
            .and_then(|b| b.join(&raw).ok())
            .map(|u| u.to_string())
    });

    let oembed_url = match fixed_oembed_endpoint {
        Some(endpoint) => build_fixed_oembed_url(endpoint, url),
        None => extract_oembed_link(&html, base.as_ref()),
    };
    let (embed_src, embed_type) = match oembed_url {
        Some(oembed_url) => fetch_oembed_embed(&oembed_url)
            .await
            .unwrap_or((None, None)),
        None => (None, None),
    };

    if title.is_none() && embed_src.is_none() {
        return Ok(None);
    }

    Ok(Some(OgpData {
        title: title.unwrap_or_default(),
        description,
        thumbnail_url,
        embed_src,
        embed_type,
    }))
}

/// 管理者設定の固定oEmbedエンドポイント（例: `https://vimeo.com/api/oembed.json`）に
/// 対象URLを`url`クエリパラメータとして付与する（oEmbed仕様の標準的な呼び出し形）。
fn build_fixed_oembed_url(endpoint: &str, target_url: &str) -> Option<Url> {
    let mut u = Url::parse(endpoint).ok()?;
    u.query_pairs_mut()
        .append_pair("url", target_url)
        .append_pair("format", "json");
    Some(u)
}

/// `<link rel="alternate" type=".../json+oembed" href="...">`（属性順序不同）から
/// oEmbedエンドポイントURLを抽出する。`type`は仕様上`application/json+oembed`だが、
/// SoundCloud等が非準拠の`text/json+oembed`を使うため、プレフィックスは問わず
/// `json+oembed`部分一致で判定する（`.../xml+oembed`は対象外、5サービスとも
/// JSON形式のため）。`extract_og_content`のような属性順の全パターン列挙ではなく、
/// `<link ...>`タグ全体を1つ取り出してからタグ内でrel/type/hrefを個別に判定する
/// （rel・type・hrefの出現順序は不定なため）。
fn extract_oembed_link(html: &str, base: Option<&Url>) -> Option<Url> {
    static LINK_TAG_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static HREF_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let link_tag_re = LINK_TAG_RE.get_or_init(|| regex::Regex::new(r#"<link\b[^>]*>"#).unwrap());
    let href_re = HREF_RE.get_or_init(|| regex::Regex::new(r#"href=["']([^"']*)["']"#).unwrap());

    for tag in link_tag_re.find_iter(html) {
        let tag_str = tag.as_str();
        let has_rel_alternate =
            tag_str.contains(r#"rel="alternate""#) || tag_str.contains(r#"rel='alternate'"#);
        let has_oembed_type = tag_str.contains("json+oembed");
        if has_rel_alternate && has_oembed_type {
            if let Some(href) = href_re.captures(tag_str).and_then(|c| c.get(1)) {
                let raw = html_escape::decode_html_entities(href.as_str()).into_owned();
                return base
                    .and_then(|b| b.join(&raw).ok())
                    .or_else(|| Url::parse(&raw).ok());
            }
        }
    }
    None
}

/// oEmbedエンドポイントからJSONを取得し、`html`フィールドからiframe srcを、
/// `type`フィールドをそのまま抽出する。SSRF対策は`fetch_validated_with_accept`を
/// 再利用する（oEmbed discoveryで見つかった`href`も外部サイトが自由に指定できる値のため、
/// OGPページ取得と同じ検証が必要）。
async fn fetch_oembed_embed(
    oembed_url: &Url,
) -> Result<(Option<String>, Option<String>), FetchError> {
    let (bytes, _) = fetch_validated_with_accept(
        oembed_url.as_str(),
        &["application/json", "text/json"],
        "application/json",
    )
    .await?;
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| FetchError::UpstreamError)?;
    let embed_type = json
        .get("type")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let embed_src = json
        .get("html")
        .and_then(|v| v.as_str())
        .and_then(extract_iframe_src);
    Ok((embed_src, embed_type))
}

/// oEmbedレスポンスの`html`フィールドからiframeのsrc属性を抽出する。対象5サービスとも
/// 単一の`<iframe>`しか返さない前提だが、最初の1マッチだけを採用する（`find_iter`による
/// 全マッチ列挙ではなく`captures`による単発マッチ）ことで、万一複数`<iframe>`が混入した
/// レスポンスが返ってきても最初の要素だけを信頼し、誤ったiframe srcの採用を防ぐ。
fn extract_iframe_src(html_fragment: &str) -> Option<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re =
        RE.get_or_init(|| regex::Regex::new(r#"<iframe\b[^>]*\bsrc=["']([^"']+)["']"#).unwrap());
    re.captures(html_fragment)
        .and_then(|c| c.get(1))
        .map(|m| html_escape::decode_html_entities(m.as_str()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{decode_html_body, ensure_public_url, extract_body_urls, is_public_ip};

    #[test]
    fn extract_body_urls_dedupes_and_preserves_order() {
        let text =
            "見て https://a.example/x これも https://b.example/y そしてまた https://a.example/x";
        assert_eq!(
            extract_body_urls(text),
            vec!["https://a.example/x", "https://b.example/y"]
        );
    }

    #[test]
    fn extract_body_urls_caps_at_five() {
        let text = (0..8)
            .map(|i| format!("https://example.com/{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(extract_body_urls(&text).len(), 5);
    }

    #[test]
    fn extract_body_urls_empty_when_no_url() {
        assert!(extract_body_urls("こんにちは").is_empty());
    }

    #[test]
    fn decode_html_body_uses_http_header_charset() {
        let (bytes, _, _) = encoding_rs::EUC_JP.encode("<html><body>楽天市場</body></html>");
        let html = decode_html_body(&bytes, "text/html; charset=EUC-JP");
        assert!(html.contains("楽天市場"));
    }

    #[test]
    fn decode_html_body_uses_meta_http_equiv_when_header_missing() {
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(
            r#"<html><head><meta http-equiv="Content-Type" content="text/html; charset=Shift_JIS"></head><body>日本語</body></html>"#,
        );
        let html = decode_html_body(&bytes, "text/html");
        assert!(html.contains("日本語"));
    }

    #[test]
    fn decode_html_body_uses_meta_charset_when_header_missing() {
        let (bytes, _, _) = encoding_rs::SHIFT_JIS
            .encode(r#"<html><head><meta charset="Shift_JIS"></head><body>日本語</body></html>"#);
        let html = decode_html_body(&bytes, "text/html");
        assert!(html.contains("日本語"));
    }

    #[test]
    fn decode_html_body_defaults_to_utf8() {
        let html = decode_html_body("<html><body>hello</body></html>".as_bytes(), "text/html");
        assert!(html.contains("hello"));
    }

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
            "::127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
            "2001:db8::1",
            "192.0.2.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
        assert!(is_public_ip("64:ff9b::808:808".parse().unwrap()));
        assert!(is_public_ip("2002:808:808::".parse().unwrap()));
    }

    #[test]
    fn ensure_public_url_rejects_private_ip_literals() {
        for url in [
            "http://127.0.0.1/users/a",
            "http://[::1]:3000/",
            "https://169.254.169.254/latest/meta-data",
            "http://10.1.2.3/inbox",
            "http://[::ffff:192.168.0.1]/",
        ] {
            assert!(ensure_public_url(url).is_err(), "{url}");
        }
        assert!(ensure_public_url("file:///etc/passwd").is_err());
        assert!(ensure_public_url("https://mastodon.social/users/a").is_ok());
        assert!(ensure_public_url("https://8.8.8.8/").is_ok());
    }
}
