//! AT Protocol DID解決（サービス間認証JWTの検証鍵取得用）。
//!
//! `did:plc:` は `plc.directory`、`did:web:` は `https://{domain}/.well-known/did.json`
//! からDIDドキュメントを取得し、`verificationMethod` の `#atproto` エントリの
//! `publicKeyMultibase`（`did:key`と同じmulticodec形式、"did:key:"接頭辞なし）を
//! P-256公開鍵にデコードする。`crates/seiran-common/src/atp/plc.rs`の
//! `p256_to_did_key`のエンコードを逆にたどる処理にあたる。

use serde::Deserialize;

use super::plc::plc_directory_base_url;
use crate::net::{fetch_json_validated, validate_url, FetchError};

#[derive(Debug, thiserror::Error)]
pub enum DidResolveError {
    #[error("DIDドキュメント取得失敗: {0}")]
    Fetch(String),
    #[error("DIDドキュメント解析失敗: {0}")]
    Parse(String),
    #[error("#atproto verificationMethod が見つかりません")]
    NoVerificationMethod,
    #[error("service が見つかりません: {0}")]
    NoService(String),
    #[error("公開鍵デコード失敗: {0}")]
    KeyDecode(String),
    #[error("非対応のDIDメソッド: {0}")]
    UnsupportedMethod(String),
}

#[derive(Deserialize)]
struct DidDocument {
    #[serde(rename = "verificationMethod", default)]
    verification_method: Vec<VerificationMethod>,
}

#[derive(Deserialize)]
struct VerificationMethod {
    id: String,
    #[serde(rename = "publicKeyMultibase")]
    public_key_multibase: Option<String>,
}

/// DIDドキュメントを生JSONのまま取得する（`com.atproto.repo.describeRepo` の `didDoc` 用）。
/// 検証鍵の抽出は行わず、取得したドキュメントをそのまま返す。
///
/// [SEC-3] `did:web:` はDID主体が指定する任意ドメインから、`did:plc:` もPLCディレクトリに
/// 自己申告された内容を取得するため、取得先URLは実質的に相手（リクエスト送信者が名乗る
/// DID）が制御できる。`fetch_json_validated`でprivate/loopback/link-local等のIPを拒否し、
/// DNS rebinding・リダイレクト先双方を検証してからでないとSSRFの踏み台になる
/// （`resolve_atproto_verification_key`は未認証の受信リクエスト署名検証からも呼ばれるため、
/// 未認証の第三者から到達可能な経路でもある）。
pub async fn fetch_raw_did_document(did: &str) -> Result<serde_json::Value, DidResolveError> {
    let doc_url = if did.starts_with("did:plc:") {
        format!("{}/{}", plc_directory_base_url(), did)
    } else if let Some(domain) = did.strip_prefix("did:web:") {
        let domain = domain.replace(':', "/");
        format!("https://{}/.well-known/did.json", domain)
    } else {
        return Err(DidResolveError::UnsupportedMethod(did.to_string()));
    };

    fetch_json_validated(&doc_url)
        .await
        .map_err(|e| match e {
            FetchError::PrivateAddress | FetchError::InvalidUrl | FetchError::InvalidRedirect => {
                DidResolveError::Fetch(format!("SSRF対策により拒否: {}", e))
            }
            other => DidResolveError::Fetch(other.to_string()),
        })
}

/// DIDドキュメントの `service` 配列から `#<service_id>` に対応する `serviceEndpoint` を取得する
/// （`atproto-proxy` ヘッダー経由のXRPCプロキシ、`crates/seiran-api/src/handlers/xrpc/proxy.rs`用）。
/// `service[].id` は実装により相対フラグメント（`#bsky_appview`）と完全修飾
/// （`did:web:api.bsky.app#bsky_appview`）の両方がありうるため両対応する。
/// 解決済み`serviceEndpoint`と、SSRF検証時にDNS解決したIPアドレス（呼び出し側が接続先を
/// 固定し、検証後のDNS rebindingを防ぐために使う）。
pub struct ResolvedServiceEndpoint {
    pub url: String,
    pub addresses: Vec<std::net::SocketAddr>,
}

pub async fn resolve_service_endpoint(
    did: &str,
    service_id: &str,
) -> Result<ResolvedServiceEndpoint, DidResolveError> {
    let doc = fetch_raw_did_document(did).await?;
    let fragment = format!("#{}", service_id);
    let services = doc
        .get("service")
        .and_then(|v| v.as_array())
        .ok_or_else(|| DidResolveError::NoService(fragment.clone()))?;
    let qualified = format!("{}{}", did, fragment);

    let endpoint = services
        .iter()
        .find(|s| {
            let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
            id == fragment || id == qualified
        })
        .and_then(|s| s.get("serviceEndpoint"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| DidResolveError::NoService(fragment.clone()))?;

    // [SEC-3] `serviceEndpoint`自体もDID主体が自由に書ける値であり、ドキュメント取得が
    // 安全でも、続けてXRPCリクエストを転送する先（`handlers/xrpc/proxy.rs`）としては
    // 未検証。private/loopback/link-local等を拒否し、DNS解決した実IPを呼び出し側へ返す
    // ことで、実際の転送でも同じIPへ接続を固定できる（検証後の再解決によるDNS
    // rebindingを防ぐ）。
    let (_, addresses) = validate_url(&endpoint)
        .await
        .map_err(|_| DidResolveError::NoService(fragment))?;
    Ok(ResolvedServiceEndpoint {
        url: endpoint,
        addresses,
    })
}

/// DIDを解決してAT Protocol検証鍵（P-256公開鍵）を取得する。
///
/// [SEC-3] 未認証の受信リクエスト（AP/AT署名検証）からも呼ばれる経路のため、
/// `fetch_raw_did_document`のSSRF対策（private/loopback/link-local拒否・
/// DNS rebinding対策）を必ず経由する。以前は独自に`reqwest::Client`で直接
/// フェッチしており、この経路だけガードが掛かっていなかった。
pub async fn resolve_atproto_verification_key(
    did: &str,
) -> Result<p256::ecdsa::VerifyingKey, DidResolveError> {
    let doc_value = fetch_raw_did_document(did).await?;
    let doc: DidDocument = serde_json::from_value(doc_value)
        .map_err(|e| DidResolveError::Parse(e.to_string()))?;

    let vm = doc
        .verification_method
        .iter()
        .find(|vm| vm.id.ends_with("#atproto"))
        .ok_or(DidResolveError::NoVerificationMethod)?;
    let multibase = vm
        .public_key_multibase
        .as_deref()
        .ok_or(DidResolveError::NoVerificationMethod)?;

    decode_p256_multikey(multibase)
}

/// `publicKeyMultibase`（`z`接頭辞のbase58btc、multicodec p256-pub = varint [0x80, 0x24]
/// + SEC1圧縮点33バイト）をP-256公開鍵にデコードする。
fn decode_p256_multikey(multibase: &str) -> Result<p256::ecdsa::VerifyingKey, DidResolveError> {
    let encoded = multibase
        .strip_prefix('z')
        .ok_or_else(|| DidResolveError::KeyDecode("multibaseのz接頭辞がありません".to_owned()))?;
    let bytes = bs58::decode(encoded)
        .into_vec()
        .map_err(|e| DidResolveError::KeyDecode(e.to_string()))?;
    let point_bytes = bytes.strip_prefix(&[0x80u8, 0x24u8]).ok_or_else(|| {
        DidResolveError::KeyDecode("p256-pub multicodec接頭辞と一致しません".to_owned())
    })?;
    p256::ecdsa::VerifyingKey::from_sec1_bytes(point_bytes)
        .map_err(|e| DidResolveError::KeyDecode(e.to_string()))
}
