//! 自ドメイン外のハンドル解決（`com.atproto.identity.resolveHandle` の他PDS向けフォールバック）。
//!
//! AT Protocol の PDS は本来、自ドメインのユーザーに限らず任意のハンドルを
//! DNS TXT (`_atproto.{handle}`) または HTTP well-known (`https://{handle}/.well-known/atproto-did`)
//! 経由で解決できる必要がある。bsky.app 等のクライアントはログイン中のPDSに対して
//! 任意ハンドルの `resolveHandle` を投げてくるため、自ドメイン外を無条件で拒否すると
//! そのクライアント側の処理が異常な状態に陥る（フリーズ等）。
//!
//! DNS TXT の取得は名前解決ライブラリを新規に依存追加せず、Cloudflare の
//! DNS-over-HTTPS JSON API（`https://cloudflare-dns.com/dns-query`）を利用する。

use std::time::Duration;

use serde::Deserialize;

/// 外部解決に許容する最大待機時間。bsky.app 等の呼び出し元をブロックし続けないよう短く保つ。
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct DohResponse {
    #[serde(rename = "Answer", default)]
    answer: Vec<DohAnswer>,
}

#[derive(Deserialize)]
struct DohAnswer {
    data: String,
}

/// `handle` を DNS TXT または HTTP well-known 経由で DID に解決する。
/// 両方失敗、またはタイムアウトした場合は `None`。
pub async fn resolve_external_handle(handle: &str, http: &reqwest::Client) -> Option<String> {
    let (dns, well_known) =
        tokio::join!(resolve_via_dns_txt(handle, http), resolve_via_well_known(handle, http));
    dns.or(well_known)
}

fn is_valid_did(did: &str) -> bool {
    did.starts_with("did:plc:") || did.starts_with("did:web:")
}

async fn resolve_via_dns_txt(handle: &str, http: &reqwest::Client) -> Option<String> {
    let url = format!("https://cloudflare-dns.com/dns-query?name=_atproto.{handle}&type=TXT");
    let resp = http
        .get(&url)
        .header("Accept", "application/dns-json")
        .timeout(RESOLVE_TIMEOUT)
        .send()
        .await
        .ok()?;
    let body: DohResponse = resp.json().await.ok()?;

    body.answer.into_iter().find_map(|a| {
        let unquoted = a.data.trim_matches('"');
        let did = unquoted.strip_prefix("did=")?;
        is_valid_did(did).then(|| did.to_string())
    })
}

async fn resolve_via_well_known(handle: &str, http: &reqwest::Client) -> Option<String> {
    let url = format!("https://{handle}/.well-known/atproto-did");
    let resp = http.get(&url).timeout(RESOLVE_TIMEOUT).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    let did = body.trim();
    is_valid_did(did).then(|| did.to_string())
}
