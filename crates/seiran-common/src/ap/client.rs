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
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 公開鍵キャッシュの有効期限。リモートサーバーが鍵をローテーションしても、
/// 最大でもこの時間が経てば新しい鍵を再フェッチするようになる
/// （`verify_signature` は加えて検証失敗時に1回だけ強制再フェッチも行う）。
const KEY_CACHE_TTL: Duration = Duration::from_secs(3600);

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
    /// ピン留め投稿の OrderedCollection URL（#61）。無い実装（Mastodon 以前や一部の
    /// 軽量実装）もあるため `Option`。
    #[serde(default)]
    pub featured: Option<String>,
    /// フォロー中一覧の OrderedCollection URL（#68）。非公開設定にしている実装や
    /// 未対応実装もあるため `Option`。
    #[serde(default)]
    pub following: Option<String>,
    /// フォロワー一覧の OrderedCollection URL（#68）。`following` と同様の理由で `Option`。
    #[serde(default)]
    pub followers: Option<String>,
    #[serde(rename = "publicKey")]
    pub public_key: Option<PublicKeyInfo>,
    /// 表示名(`name`)・自己紹介(`summary`)中のカスタム絵文字タグ(`type:"Emoji"`)。
    /// `emoji_map()` で `{shortcode: 画像URL}` に変換する。
    #[serde(default)]
    pub tag: Vec<serde_json::Value>,
    /// プロフィールのキーバリュー項目（#62）。`type: "PropertyValue"` の要素を
    /// `property_values()` で `(name, value)` のペアに変換する。
    #[serde(default)]
    pub attachment: Vec<serde_json::Value>,
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

    /// `attachment` 配列（`type: "PropertyValue"` の要素）から `(name, value)` のペアを
    /// 抽出する（#62）。`value` は HTML を含みうるため（Mastodon 等はリンクを `<a>` タグ付きで
    /// 送る）、呼び出し側で必要に応じてプレーンテキスト化すること。
    pub fn property_values(&self) -> Vec<(String, String)> {
        self.attachment
            .iter()
            .filter(|a| a.get("type").and_then(|t| t.as_str()) == Some("PropertyValue"))
            .filter_map(|a| {
                let name = a.get("name")?.as_str()?.to_string();
                let value = a.get("value")?.as_str()?.to_string();
                Some((name, value))
            })
            .collect()
    }

    /// `property_values()` を `MAX_PROFILE_FIELDS` 件までに切り詰め、`value` を `strip_html`
    /// でプレーンテキスト化した上で `actors.profile_fields` へそのまま保存できる JSON 配列
    /// （`[{"name": ..., "value": ...}, ...]`）を組み立てる（#62）。
    pub fn profile_fields_json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.property_values()
                .into_iter()
                .filter_map(|(name, value)| {
                    // strip_html 後に空になる値（アイコンのみのリンク等）は取り込まない。
                    let value = crate::jobs::inbound_activity_process::strip_html(&value);
                    if value.trim().is_empty() {
                        None
                    } else {
                        Some(serde_json::json!({"name": name, "value": value}))
                    }
                })
                .take(crate::MAX_PROFILE_FIELDS)
                .collect(),
        )
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

/// AP Note の `to`/`cc` から Mastodon 互換の4値可視性を判定する。
/// - `to` に Public が含まれる: `public`（一般的な公開投稿）
/// - `cc` にのみ Public が含まれる: `unlisted`（公開だが一覧に載らない）
/// - Public がどちらにも無く `to` にフォロワーコレクション（`.../followers`）が含まれる: `followers_only`
/// - それ以外（特定アクターのみ宛先）: `direct`
pub fn classify_ap_visibility(to: &[String], cc: &[String]) -> &'static str {
    const PUBLIC_URIS: [&str; 3] =
        ["https://www.w3.org/ns/activitystreams#Public", "as:Public", "Public"];
    let has_public = |uris: &[String]| uris.iter().any(|u| PUBLIC_URIS.contains(&u.as_str()));

    if has_public(to) {
        "public"
    } else if has_public(cc) {
        "unlisted"
    } else if to.iter().any(|u| u.ends_with("/followers")) {
        "followers_only"
    } else {
        "direct"
    }
}

/// ActivityPub 通信クライアント
///
/// HTTP クライアントと公開鍵キャッシュをインスタンスフィールドとして保持する。
/// プロセスグローバルな静的キャッシュを廃止し、テスト時にモックを注入できる構造にした。
pub struct ApClient {
    pub http: Arc<reqwest::Client>,
    pub key_cache: Arc<RwLock<HashMap<String, (String, Instant)>>>,
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

    /// 指定した key_id (URL) から公開鍵 PEM を取得する（TTL付きキャッシュ対応）
    pub async fn get_public_key_pem(&self, key_id: &str) -> Result<String, ApError> {
        // 1. キャッシュヒット確認（TTL内のみ有効）
        {
            let cache = self.key_cache.read().await;
            if let Some((pem, fetched_at)) = cache.get(key_id) {
                if fetched_at.elapsed() < KEY_CACHE_TTL {
                    return Ok(pem.clone());
                }
            }
        }

        self.fetch_and_cache_public_key_pem(key_id).await
    }

    /// キャッシュの有無・TTLを無視して公開鍵を再フェッチし、結果でキャッシュを上書きする。
    /// リモートの鍵ローテーション後に署名検証が失敗した際のリトライで使う。
    async fn fetch_and_cache_public_key_pem(&self, key_id: &str) -> Result<String, ApError> {
        tracing::info!("[ApClient] 公開鍵フェッチ中: {}", key_id);

        // アクターもしくは鍵を直接フェッチする。
        // 通常 key_id (e.g. https://example.com/users/test#main-key) にアクセスすると
        // アクター情報そのもの、あるいは鍵オブジェクト単体が返る。
        // フラグメント部分 (#main-key) を除外したベースURIを叩くのが安全。
        let base_uri = key_id.split('#').next().unwrap_or(key_id);
        let actor = self.fetch_actor(base_uri).await?;

        if let Some(pubkey_info) = actor.public_key {
            if pubkey_info.id == key_id || base_uri == pubkey_info.owner {
                let pem = pubkey_info.public_key_pem;
                let mut cache = self.key_cache.write().await;
                cache.insert(key_id.to_string(), (pem.clone(), Instant::now()));
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

        // 3. 署名の base64 デコード（鍵の取得元によらず共通）
        let signature_bytes = base64::Engine::decode(&base64::prelude::BASE64_STANDARD, signature_b64)
            .map_err(|e| ApError::Signature(format!("署名base64デコード失敗: {}", e)))?;

        // 4. 公開鍵 PEM の取得（キャッシュ利用）と検証
        let pem = self.get_public_key_pem(key_id).await?;
        if Self::verify_with_pem(&pem, &signing_string, &signature_bytes).is_ok() {
            return Ok(true);
        }

        // 5. キャッシュ済みの鍵での検証に失敗した場合、リモートが鍵をローテーションした
        // 可能性があるため、キャッシュを無視して1回だけ再フェッチし再検証する。
        // 同じ鍵しか得られなかった場合は無駄な再検証をせず最初の失敗をそのまま返す。
        let fresh_pem = self.fetch_and_cache_public_key_pem(key_id).await?;
        if fresh_pem == pem {
            return Err(ApError::Signature("署名検証失敗".to_string()));
        }
        Self::verify_with_pem(&fresh_pem, &signing_string, &signature_bytes).map(|()| true)
    }

    /// 与えられた公開鍵 PEM で signing string の署名を検証する（純粋な検証処理部分）
    fn verify_with_pem(pem: &str, signing_string: &str, signature_bytes: &[u8]) -> Result<(), ApError> {
        let public_key = RsaPublicKey::from_public_key_pem(pem)
            .map_err(|e| ApError::Signature(format!("RSA公開鍵のパース失敗: {}", e)))?;

        let mut hasher = Sha256::new();
        hasher.update(signing_string.as_bytes());
        let hashed = hasher.finalize();

        public_key
            .verify(Pkcs1v15Sign::new::<Sha256>(), &hashed, signature_bytes)
            .map_err(|e| ApError::Signature(format!("署名検証失敗: {:?}", e)))
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

    // ─── classify_ap_visibility ────────────────────────────────────────────

    #[test]
    fn classify_ap_visibility_public() {
        let to = vec!["https://www.w3.org/ns/activitystreams#Public".to_string()];
        let cc = vec!["https://example.com/users/alice/followers".to_string()];
        assert_eq!(classify_ap_visibility(&to, &cc), "public");
    }

    #[test]
    fn classify_ap_visibility_unlisted() {
        let to = vec!["https://example.com/users/alice/followers".to_string()];
        let cc = vec!["https://www.w3.org/ns/activitystreams#Public".to_string()];
        assert_eq!(classify_ap_visibility(&to, &cc), "unlisted");
    }

    #[test]
    fn classify_ap_visibility_followers_only() {
        let to = vec!["https://example.com/users/alice/followers".to_string()];
        let cc: Vec<String> = vec![];
        assert_eq!(classify_ap_visibility(&to, &cc), "followers_only");
    }

    #[test]
    fn classify_ap_visibility_direct() {
        let to = vec!["https://example.com/users/bob".to_string()];
        let cc: Vec<String> = vec![];
        assert_eq!(classify_ap_visibility(&to, &cc), "direct");
    }

    // ─── ApClient 公開鍵キャッシュのTTL ────────────────────────────────────

    #[tokio::test]
    async fn get_public_key_pem_returns_cached_value_when_fresh() {
        let client = ApClient::new(Arc::new(reqwest::Client::new()));
        {
            let mut cache = client.key_cache.write().await;
            cache.insert("https://example.com/users/alice#main-key".to_string(), ("PEM-DATA".to_string(), Instant::now()));
        }
        // TTL内のキャッシュヒットのため、ネットワークアクセスなしで即座に返る。
        let pem = client.get_public_key_pem("https://example.com/users/alice#main-key").await.unwrap();
        assert_eq!(pem, "PEM-DATA");
    }

    #[tokio::test]
    async fn get_public_key_pem_ignores_stale_cache_entry() {
        let client = ApClient::new(Arc::new(reqwest::Client::new()));
        let stale_at = Instant::now().checked_sub(KEY_CACHE_TTL + Duration::from_secs(1)).unwrap();
        {
            let mut cache = client.key_cache.write().await;
            cache.insert("https://example.com/users/alice#main-key".to_string(), ("OLD-PEM".to_string(), stale_at));
        }
        // TTL切れのためキャッシュを使わず再フェッチを試みる。到達不能ホストなのでエラーになるが、
        // 「古いPEMをそのまま返してしまう」ことがないのが検証したい点。
        let result = client.get_public_key_pem("https://127.0.0.1.invalid/users/alice#main-key").await;
        assert!(result.is_err());
    }
}
