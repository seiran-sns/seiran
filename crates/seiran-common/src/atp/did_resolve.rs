//! AT Protocol DID解決（サービス間認証JWTの検証鍵取得用）。
//!
//! `did:plc:` は `plc.directory`、`did:web:` は `https://{domain}/.well-known/did.json`
//! からDIDドキュメントを取得し、`verificationMethod` の `#atproto` エントリの
//! `publicKeyMultibase`（`did:key`と同じmulticodec形式、"did:key:"接頭辞なし）を
//! P-256公開鍵にデコードする。`crates/seiran-common/src/atp/plc.rs`の
//! `p256_to_did_key`のエンコードを逆にたどる処理にあたる。

use serde::Deserialize;

use super::plc::plc_directory_base_url;

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
pub async fn fetch_raw_did_document(
    did: &str,
    http: &reqwest::Client,
) -> Result<serde_json::Value, DidResolveError> {
    let doc_url = if did.starts_with("did:plc:") {
        format!("{}/{}", plc_directory_base_url(), did)
    } else if let Some(domain) = did.strip_prefix("did:web:") {
        let domain = domain.replace(':', "/");
        format!("https://{}/.well-known/did.json", domain)
    } else {
        return Err(DidResolveError::UnsupportedMethod(did.to_string()));
    };

    let resp = http
        .get(&doc_url)
        .send()
        .await
        .map_err(|e| DidResolveError::Fetch(e.to_string()))?;
    resp.json()
        .await
        .map_err(|e| DidResolveError::Parse(e.to_string()))
}

/// DIDドキュメントの `service` 配列から `#<service_id>` に対応する `serviceEndpoint` を取得する
/// （`atproto-proxy` ヘッダー経由のXRPCプロキシ、`crates/seiran-api/src/handlers/xrpc/proxy.rs`用）。
/// `service[].id` は実装により相対フラグメント（`#bsky_appview`）と完全修飾
/// （`did:web:api.bsky.app#bsky_appview`）の両方がありうるため両対応する。
pub async fn resolve_service_endpoint(
    did: &str,
    service_id: &str,
    http: &reqwest::Client,
) -> Result<String, DidResolveError> {
    let doc = fetch_raw_did_document(did, http).await?;
    let fragment = format!("#{}", service_id);
    let services = doc
        .get("service")
        .and_then(|v| v.as_array())
        .ok_or_else(|| DidResolveError::NoService(fragment.clone()))?;
    let qualified = format!("{}{}", did, fragment);

    services
        .iter()
        .find(|s| {
            let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
            id == fragment || id == qualified
        })
        .and_then(|s| s.get("serviceEndpoint"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or(DidResolveError::NoService(fragment))
}

/// DIDを解決してAT Protocol検証鍵（P-256公開鍵）を取得する。
pub async fn resolve_atproto_verification_key(
    did: &str,
    http: &reqwest::Client,
) -> Result<p256::ecdsa::VerifyingKey, DidResolveError> {
    let doc_url = if did.starts_with("did:plc:") {
        format!("{}/{}", plc_directory_base_url(), did)
    } else if let Some(domain) = did.strip_prefix("did:web:") {
        // did:web の `:` はパス区切りにデコードされる（ポート番号を除く。今回は未対応）
        let domain = domain.replace(':', "/");
        format!("https://{}/.well-known/did.json", domain)
    } else {
        return Err(DidResolveError::UnsupportedMethod(did.to_string()));
    };

    let resp = http
        .get(&doc_url)
        .send()
        .await
        .map_err(|e| DidResolveError::Fetch(e.to_string()))?;
    let doc: DidDocument = resp
        .json()
        .await
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
