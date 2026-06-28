//! DID 解決モジュール
//!
//! `did:plc` は plc.directory へ、`did:web` は .well-known/did.json へ問い合わせ、
//! DID ドキュメントを返す。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidDocument {
    pub id: String,
    #[serde(default)]
    pub also_known_as: Vec<String>,
    #[serde(default)]
    pub service: Vec<DidService>,
    #[serde(default)]
    pub verification_method: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidService {
    pub id: String,
    #[serde(rename = "type")]
    pub service_type: String,
    /// 文字列または `{ "url": "..." }` オブジェクトの両形式を許容
    pub service_endpoint: serde_json::Value,
}

impl DidDocument {
    /// PDS エンドポイント URL を取得する
    pub fn pds_endpoint(&self) -> Option<String> {
        self.service
            .iter()
            .find(|s| s.service_type == "AtprotoPersonalDataServer")
            .and_then(|s| match &s.service_endpoint {
                serde_json::Value::String(url) => Some(url.clone()),
                serde_json::Value::Object(obj) => obj
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                _ => None,
            })
    }

    /// `alsoKnownAs` から AT Protocol ハンドルを取得する（`at://` プレフィックスを除去）
    pub fn handle(&self) -> Option<&str> {
        self.also_known_as
            .iter()
            .find(|s| s.starts_with("at://"))
            .map(|s| s.trim_start_matches("at://"))
    }
}

/// DID を解決して DID ドキュメントを返す
pub async fn resolve_did(did: &str) -> Result<DidDocument, String> {
    let url = if did.starts_with("did:plc:") {
        format!("https://plc.directory/{}", did)
    } else if let Some(rest) = did.strip_prefix("did:web:") {
        // did:web:example.com           → https://example.com/.well-known/did.json
        // did:web:example.com:path:sub  → https://example.com/path/sub/did.json
        if let Some(colon_pos) = rest.find(':') {
            let domain = &rest[..colon_pos];
            let path = rest[colon_pos + 1..].replace(':', "/");
            format!("https://{}/{}/did.json", domain, path)
        } else {
            format!("https://{}/.well-known/did.json", rest)
        }
    } else {
        return Err(format!("未対応のDIDメソッド: {}", did));
    };

    let client = reqwest::Client::builder()
        .user_agent("seiran-federation/0.1.0")
        .build()
        .map_err(|e| format!("HTTPクライアント初期化失敗: {}", e))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("DID解決HTTPエラー: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("DID解決失敗 ({}): {}", resp.status(), did));
    }

    resp.json::<DidDocument>()
        .await
        .map_err(|e| format!("DIDドキュメントパースエラー: {}", e))
}
