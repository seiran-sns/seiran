//! AT Protocol DID解決（サービス間認証JWTの検証鍵取得用）。
//!
//! `did:plc:` は `plc.directory`、`did:web:` は `https://{domain}/.well-known/did.json`
//! からDIDドキュメントを取得し、`verificationMethod` の `#atproto` エントリの
//! `publicKeyMultibase`（`did:key`と同じmulticodec形式、"did:key:"接頭辞なし）を
//! P-256公開鍵にデコードする。`crates/seiran-common/src/atp/plc.rs`の
//! `p256_to_did_key`のエンコードを逆にたどる処理にあたる。

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum DidResolveError {
    #[error("DIDドキュメント取得失敗: {0}")]
    Fetch(String),
    #[error("DIDドキュメント解析失敗: {0}")]
    Parse(String),
    #[error("#atproto verificationMethod が見つかりません")]
    NoVerificationMethod,
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

/// DIDを解決してAT Protocol検証鍵（P-256公開鍵）を取得する。
pub async fn resolve_atproto_verification_key(
    did: &str,
    http: &reqwest::Client,
) -> Result<p256::ecdsa::VerifyingKey, DidResolveError> {
    let doc_url = if did.starts_with("did:plc:") {
        format!("https://plc.directory/{}", did)
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
    let point_bytes = bytes
        .strip_prefix(&[0x80u8, 0x24u8])
        .ok_or_else(|| DidResolveError::KeyDecode("p256-pub multicodec接頭辞と一致しません".to_owned()))?;
    p256::ecdsa::VerifyingKey::from_sec1_bytes(point_bytes)
        .map_err(|e| DidResolveError::KeyDecode(e.to_string()))
}
