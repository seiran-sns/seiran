//! ActivityPub クライアント ＆ HTTP Signatures 署名検証モジュール
//!
//! リモートアクタードキュメントの取得、公開鍵（RSA）のフェッチとキャッシュ、
//! および受信リクエストの HTTP Signatures 署名検証を行う。

use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::{Pkcs1v15Sign, RsaPrivateKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// AP 通信エラー
#[derive(Debug, thiserror::Error)]
pub enum ApError {
    #[error("HTTP エラー: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON パースエラー: {0}")]
    Json(#[from] serde_json::Error),
    #[error("署名エラー: {0}")]
    Signature(String),
    #[error("アクター取得失敗: {0}")]
    FetchActor(String),
    #[error("{0}")]
    Other(String),
}

/// 後方互換性のため `Result<_, String>` コンテキストで `?` が使えるようにする
impl From<ApError> for String {
    fn from(e: ApError) -> Self {
        e.to_string()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PublicKeyInfo {
    pub id: String,
    pub owner: String,
    #[serde(rename = "publicKeyPem")]
    pub public_key_pem: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApActor {
    pub id: String,
    #[serde(rename = "type")]
    pub actor_type: String,
    #[serde(rename = "preferredUsername")]
    pub preferred_username: Option<String>,
    pub name: Option<String>,
    pub summary: Option<String>,
    /// アバター画像。実装により object / array / 欠落があり得るため Value で受ける
    /// （`avatar_url()` で URL を抽出する）。
    #[serde(default)]
    pub icon: Option<serde_json::Value>,
    pub inbox: Option<String>,
    pub outbox: Option<String>,
    #[serde(rename = "publicKey")]
    pub public_key: Option<PublicKeyInfo>,
    /// 表示名(`name`)・自己紹介(`summary`)中のカスタム絵文字タグ(`type:"Emoji"`)。
    /// `emoji_map()` で `{shortcode: 画像URL}` に変換する。
    #[serde(default)]
    pub tag: Vec<serde_json::Value>,
}

impl ApActor {
    /// `icon`（object または array）から最初の画像 URL を抽出する。
    pub fn avatar_url(&self) -> Option<String> {
        let v = self.icon.as_ref()?;
        let obj = if v.is_array() {
            v.as_array()?.first()?
        } else {
            v
        };
        obj.get("url")?.as_str().map(|s| s.to_string())
    }

    /// 表示名中のカスタム絵文字の shortcode→画像URLマップ。
    pub fn emoji_map(&self) -> serde_json::Value {
        build_emoji_map(&self.tag)
    }
}

/// AP の `tag` 配列（`type:"Emoji"` の要素）から `{shortcode: 画像URL}` のマップを構築する。
/// Note 本文・Person 表示名・Like/EmojiReact のいずれでも同じ形式で使われる:
/// `{"id":"...", "type":"Emoji", "name":":shortcode:", "icon":{"type":"Image","url":"..."}}`
pub fn build_emoji_map(tags: &[serde_json::Value]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for tag in tags {
        if tag["type"].as_str() != Some("Emoji") {
            continue;
        }
        if let (Some(name), Some(url)) = (tag["name"].as_str(), tag["icon"]["url"].as_str()) {
            map.insert(name.to_string(), serde_json::Value::String(url.to_string()));
        }
    }
    serde_json::Value::Object(map)
}

/// ActivityPub 通信クライアント
///
/// HTTP クライアントと公開鍵キャッシュをインスタンスフィールドとして保持する。
/// プロセスグローバルな静的キャッシュを廃止し、テスト時にモックを注入できる構造にした。
pub struct ApClient {
    pub http: Arc<reqwest::Client>,
    pub key_cache: Arc<RwLock<HashMap<String, String>>>,
}

impl ApClient {
    pub fn new(http: Arc<reqwest::Client>) -> Self {
        Self {
            http,
            key_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// リモートアクター情報を取得する
    pub async fn fetch_actor(&self, actor_uri: &str) -> Result<ApActor, ApError> {
        let res = self.http
            .get(actor_uri)
            .header("Accept", "application/activity+json, application/ld+json")
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(ApError::FetchActor(format!(
                "ステータス {}",
                res.status()
            )));
        }

        let actor = res.json::<ApActor>().await?;
        Ok(actor)
    }

    /// 指定した key_id (URL) から公開鍵 PEM を取得する（キャッシュ対応）
    pub async fn get_public_key_pem(&self, key_id: &str) -> Result<String, ApError> {
        // 1. キャッシュヒット確認
        {
            let cache = self.key_cache.read().await;
            if let Some(pem) = cache.get(key_id) {
                return Ok(pem.clone());
            }
        }

        println!("[ApClient] 公開鍵フェッチ中: {}", key_id);

        // 2. キャッシュミス時はアクターもしくは鍵を直接フェッチ
        // 通常 key_id (e.g. https://example.com/users/test#main-key) にアクセスすると
        // アクター情報そのもの、あるいは鍵オブジェクト単体が返る。
        // フラグメント部分 (#main-key) を除外したベースURIを叩くのが安全。
        let base_uri = key_id.split('#').next().unwrap_or(key_id);
        let actor = self.fetch_actor(base_uri).await?;

        if let Some(pubkey_info) = actor.public_key {
            if pubkey_info.id == key_id || base_uri == pubkey_info.owner {
                let pem = pubkey_info.public_key_pem;
                // キャッシュに書き込み
                let mut cache = self.key_cache.write().await;
                cache.insert(key_id.to_string(), pem.clone());
                return Ok(pem);
            }
        }

        Err(ApError::FetchActor(format!(
            "取得したアクタードキュメントから一致する key_id ({}) が見つかりません",
            key_id
        )))
    }

    /// HTTP Signatures の署名を検証します
    ///
    /// # 引数
    /// - `method`: リクエストメソッド (e.g. "POST")
    /// - `path`: リクエストパス (e.g. "/inbox")
    /// - `headers`: 受信した HTTP ヘッダー一覧
    /// - `signature_header`: 受信した `Signature` ヘッダーの内容
    pub async fn verify_signature(
        &self,
        method: &str,
        path: &str,
        headers: &HashMap<String, String>,
        signature_header: &str,
    ) -> Result<bool, ApError> {
        // 1. Signature ヘッダーの要素をパース
        // 例: keyId="...",algorithm="rsa-sha256",headers="...",signature="..."
        let parsed = parse_signature_header(signature_header)?;
        let key_id = parsed.get("keyId")
            .ok_or_else(|| ApError::Signature("keyId が見つかりません".to_string()))?;
        let signature_b64 = parsed.get("signature")
            .ok_or_else(|| ApError::Signature("signature が見つかりません".to_string()))?;
        let header_list_str = parsed.get("headers").cloned().unwrap_or_else(|| "date".to_string());

        // 2. 署名対象文字列 (Signing String) を構築
        let signing_string = build_signing_string(method, path, headers, &header_list_str)?;

        // 3. 公開鍵 PEM の取得
        let pem = self.get_public_key_pem(key_id).await?;

        // 4. RSA 公開鍵オブジェクトのパース
        let public_key = RsaPublicKey::from_public_key_pem(&pem)
            .map_err(|e| ApError::Signature(format!("RSA公開鍵のパース失敗: {}", e)))?;

        // 5. 署名の base64 デコード
        let signature_bytes = base64::Engine::decode(&base64::prelude::BASE64_STANDARD, signature_b64)
            .map_err(|e| ApError::Signature(format!("署名base64デコード失敗: {}", e)))?;

        // 6. SHA-256 ハッシュの計算
        let mut hasher = Sha256::new();
        hasher.update(signing_string.as_bytes());
        let hashed = hasher.finalize();

        // 7. 署名検証の実行
        let result = public_key.verify(
            Pkcs1v15Sign::new::<Sha256>(),
            &hashed,
            &signature_bytes,
        );

        match result {
            Ok(()) => Ok(true),
            Err(e) => Err(ApError::Signature(format!("署名検証失敗: {:?}", e))),
        }
    }

    /// HTTP Signatures 付きで ActivityPub エンドポイントへ POST する
    ///
    /// # 引数
    /// - `url`: 送信先 URL（相手の inbox 等）
    /// - `body`: JSON 文字列
    /// - `actor_key_id`: 署名に使うキー ID（例: `https://beta.seiran.org/users/yubaj#main-key`）
    /// - `private_key_pem`: RSA 秘密鍵 PEM
    pub async fn sign_and_post(
        &self,
        url: &str,
        body: &str,
        actor_key_id: &str,
        private_key_pem: &str,
    ) -> Result<(), ApError> {
        let now = chrono::Utc::now();
        let date_str = now.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

        let parsed_url = url::Url::parse(url)
            .map_err(|e| ApError::Other(format!("URL パースエラー: {}", e)))?;
        let host = parsed_url.host_str().unwrap_or("").to_string();
        let path = parsed_url.path().to_string();

        // Digest ヘッダー（SHA-256 of body）
        let body_hash = Sha256::digest(body.as_bytes());
        let digest = format!(
            "SHA-256={}",
            base64::Engine::encode(&base64::prelude::BASE64_STANDARD, body_hash)
        );

        // 署名対象文字列
        let signing_string = format!(
            "(request-target): post {}\nhost: {}\ndate: {}\ncontent-type: application/activity+json\ndigest: {}",
            path, host, date_str, digest
        );

        // RSA-SHA256 署名
        let private_key = RsaPrivateKey::from_pkcs8_pem(private_key_pem)
            .map_err(|e| ApError::Signature(format!("RSA 秘密鍵パース失敗: {}", e)))?;

        let mut hasher = Sha256::new();
        hasher.update(signing_string.as_bytes());
        let hashed = hasher.finalize();

        let sig_bytes = private_key
            .sign(Pkcs1v15Sign::new::<Sha256>(), &hashed)
            .map_err(|e| ApError::Signature(format!("RSA 署名失敗: {}", e)))?;

        let sig_b64 = base64::Engine::encode(&base64::prelude::BASE64_STANDARD, sig_bytes);

        let signature_header = format!(
            r#"keyId="{}",algorithm="rsa-sha256",headers="(request-target) host date content-type digest",signature="{}""#,
            actor_key_id, sig_b64
        );

        let res = self.http
            .post(url)
            .header("Date", &date_str)
            .header("Host", &host)
            .header("Content-Type", "application/activity+json")
            .header("Digest", &digest)
            .header("Signature", &signature_header)
            .body(body.to_string())
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let body_text = res.text().await.unwrap_or_default();
            return Err(ApError::Other(format!("POST レスポンスエラー {}: {}", status, body_text)));
        }

        Ok(())
    }

    /// リモート AP オブジェクト（Note 等）を URI から取得する
    pub async fn fetch_object(&self, object_uri: &str) -> Result<serde_json::Value, ApError> {
        let res = self
            .http
            .get(object_uri)
            .header("Accept", "application/activity+json, application/ld+json")
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(ApError::Other(format!(
                "オブジェクト取得失敗: ステータス {} ({})",
                res.status(),
                object_uri
            )));
        }

        let obj = res.json::<serde_json::Value>().await?;
        Ok(obj)
    }

    /// Webfinger 解決を実行する
    pub async fn resolve_webfinger(&self, username: &str, domain: &str) -> Result<String, ApError> {
        super::webfinger::resolve_webfinger_impl(&self.http, username, domain).await
    }
}

/// Signature ヘッダーを簡易パースする
fn parse_signature_header(header: &str) -> Result<HashMap<String, String>, ApError> {
    let mut map = HashMap::new();
    // カンマ区切りの key="value" パターンを取り出す
    // 簡易的にクォーテーションを考慮しつつ分割する
    let parts = header.split(',');
    for part in parts {
        let kv: Vec<&str> = part.splitn(2, '=').collect();
        if kv.len() == 2 {
            let key = kv[0].trim().to_string();
            let val = kv[1].trim().trim_matches('"').to_string();
            map.insert(key, val);
        }
    }
    Ok(map)
}

/// 署名ヘッダーに列挙された順に署名対象文字列を構築する
fn build_signing_string(
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    header_list_str: &str,
) -> Result<String, ApError> {
    let mut lines = Vec::new();
    for header_name in header_list_str.split(' ') {
        let name_lower = header_name.to_lowercase();
        if name_lower == "(request-target)" {
            lines.push(format!("(request-target): {} {}", method.to_lowercase(), path));
        } else {
            let val = headers.get(&name_lower).ok_or_else(|| {
                ApError::Signature(format!(
                    "署名対象ヘッダー \"{}\" がリクエストに見つかりません",
                    header_name
                ))
            })?;
            lines.push(format!("{}: {}", name_lower, val));
        }
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── parse_signature_header ───────────────────────────────────────────

    #[test]
    fn parse_signature_header_extracts_key_fields() {
        let header = r#"keyId="https://example.com/users/alice#main-key",algorithm="rsa-sha256",headers="(request-target) host date",signature="abc123""#;
        let map = parse_signature_header(header).unwrap();
        assert_eq!(map.get("keyId").map(|s| s.as_str()), Some("https://example.com/users/alice#main-key"));
        assert_eq!(map.get("algorithm").map(|s| s.as_str()), Some("rsa-sha256"));
        assert_eq!(map.get("headers").map(|s| s.as_str()), Some("(request-target) host date"));
        assert_eq!(map.get("signature").map(|s| s.as_str()), Some("abc123"));
    }

    #[test]
    fn parse_signature_header_single_pair() {
        let header = r#"keyId="did:example:123#key-1""#;
        let map = parse_signature_header(header).unwrap();
        assert_eq!(map.get("keyId").map(|s| s.as_str()), Some("did:example:123#key-1"));
    }

    #[test]
    fn parse_signature_header_returns_empty_on_malformed() {
        let map = parse_signature_header("no-equals-sign").unwrap();
        assert!(map.is_empty());
    }

    // ─── build_signing_string ─────────────────────────────────────────────

    #[test]
    fn build_signing_string_request_target() {
        let headers = HashMap::new();
        let result = build_signing_string("POST", "/inbox", &headers, "(request-target)").unwrap();
        assert_eq!(result, "(request-target): post /inbox");
    }

    #[test]
    fn build_signing_string_multiple_headers() {
        let mut headers = HashMap::new();
        headers.insert("host".to_string(), "example.com".to_string());
        headers.insert("date".to_string(), "Mon, 01 Jan 2024 00:00:00 GMT".to_string());
        let result = build_signing_string(
            "POST",
            "/inbox",
            &headers,
            "(request-target) host date",
        ).unwrap();
        let expected = "(request-target): post /inbox\nhost: example.com\ndate: Mon, 01 Jan 2024 00:00:00 GMT";
        assert_eq!(result, expected);
    }

    #[test]
    fn build_signing_string_method_is_lowercased() {
        let headers = HashMap::new();
        let result = build_signing_string("GET", "/users/alice", &headers, "(request-target)").unwrap();
        assert!(result.starts_with("(request-target): get "));
    }

    #[test]
    fn build_signing_string_missing_header_returns_error() {
        let headers = HashMap::new();
        let err = build_signing_string("POST", "/inbox", &headers, "host").unwrap_err();
        assert!(matches!(err, ApError::Signature(_)));
    }

    // ─── build_emoji_map ───────────────────────────────────────────

    #[test]
    fn build_emoji_map_extracts_multiple_shortcodes() {
        let tags = vec![
            serde_json::json!({
                "type": "Emoji", "name": ":blobcat:",
                "icon": { "url": "https://example.com/blobcat.png" }
            }),
            serde_json::json!({
                "type": "Emoji", "name": ":ablobcatwave:",
                "icon": { "url": "https://example.com/wave.png" }
            }),
        ];
        let map = build_emoji_map(&tags);
        assert_eq!(map[":blobcat:"], "https://example.com/blobcat.png");
        assert_eq!(map[":ablobcatwave:"], "https://example.com/wave.png");
    }

    #[test]
    fn build_emoji_map_ignores_non_emoji_tags() {
        let tags = vec![serde_json::json!({
            "type": "Mention", "name": "@alice", "href": "https://example.com/users/alice"
        })];
        assert_eq!(build_emoji_map(&tags), serde_json::json!({}));
    }

    #[test]
    fn build_emoji_map_empty_tags() {
        assert_eq!(build_emoji_map(&[]), serde_json::json!({}));
    }

    #[test]
    fn ap_actor_emoji_map_uses_tag_field() {
        let actor: ApActor = serde_json::from_value(serde_json::json!({
            "id": "https://example.com/users/alice",
            "type": "Person",
            "tag": [
                { "type": "Emoji", "name": ":blobcat:", "icon": { "url": "https://example.com/blobcat.png" } }
            ]
        })).unwrap();
        assert_eq!(actor.emoji_map()[":blobcat:"], "https://example.com/blobcat.png");
    }
}
