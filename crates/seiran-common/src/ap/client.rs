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

use std::sync::OnceLock;

static PUBLIC_KEY_CACHE: OnceLock<Arc<RwLock<HashMap<String, String>>>> = OnceLock::new();

fn get_cache() -> &'static Arc<RwLock<HashMap<String, String>>> {
    PUBLIC_KEY_CACHE.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
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
    pub inbox: Option<String>,
    pub outbox: Option<String>,
    #[serde(rename = "publicKey")]
    pub public_key: Option<PublicKeyInfo>,
}

/// リモートアクター情報を取得する
pub async fn fetch_actor(actor_uri: &str) -> Result<ApActor, String> {
    let client = reqwest::Client::builder()
        .user_agent("seiran-federation/0.1.0")
        .build()
        .map_err(|e| format!("HTTPクライアント初期化失敗: {}", e))?;

    let res = client
        .get(actor_uri)
        .header("Accept", "application/activity+json, application/ld+json")
        .send()
        .await
        .map_err(|e| format!("アクタードキュメントフェッチ失敗: {}", e))?;

    if !res.status().is_success() {
        return Err(format!(
            "アクタードキュメントフェッチエラー: ステータス {}",
            res.status()
        ));
    }

    let actor = res
        .json::<ApActor>()
        .await
        .map_err(|e| format!("アクタードキュメント JSONパース失敗: {}", e))?;

    Ok(actor)
}

/// 指定した key_id (URL) から公開鍵 PEM を取得する（キャッシュ対応）
pub async fn get_public_key_pem(key_id: &str) -> Result<String, String> {
    // 1. キャッシュヒット確認
    {
        let cache = get_cache().read().await;
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
    let actor = fetch_actor(base_uri).await?;

    if let Some(pubkey_info) = actor.public_key {
        if pubkey_info.id == key_id || base_uri == pubkey_info.owner {
            let pem = pubkey_info.public_key_pem;
            // キャッシュに書き込み
            let mut cache = get_cache().write().await;
            cache.insert(key_id.to_string(), pem.clone());
            return Ok(pem);
        }
    }

    Err(format!(
        "取得したアクタードキュメントから一致する key_id ({}) が見つかりません",
        key_id
    ))
}

/// HTTP Signatures の署名を検証します
///
/// # 引数
/// - `method`: リクエストメソッド (e.g. "POST")
/// - `path`: リクエストパス (e.g. "/inbox")
/// - `headers`: 受信した HTTP ヘッダー一覧
/// - `signature_header`: 受信した `Signature` ヘッダーの内容
pub async fn verify_signature(
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    signature_header: &str,
) -> Result<bool, String> {
    // 1. Signature ヘッダーの要素をパース
    // 例: keyId="...",algorithm="rsa-sha256",headers="...",signature="..."
    let parsed = parse_signature_header(signature_header)?;
    let key_id = parsed.get("keyId").ok_or_else(|| "keyId が見つかりません".to_string())?;
    let signature_b64 = parsed.get("signature").ok_or_else(|| "signature が見つかりません".to_string())?;
    let header_list_str = parsed.get("headers").cloned().unwrap_or_else(|| "date".to_string());

    // 2. 署名対象文字列 (Signing String) を構築
    let signing_string = build_signing_string(method, path, headers, &header_list_str)?;

    // 3. 公開鍵 PEM の取得
    let pem = get_public_key_pem(key_id).await?;

    // 4. RSA 公開鍵オブジェクトのパース
    let public_key = RsaPublicKey::from_public_key_pem(&pem)
        .map_err(|e| format!("RSA公開鍵のパース失敗: {}", e))?;

    // 5. 署名の base64 デコード
    let signature_bytes = base64::Engine::decode(&base64::prelude::BASE64_STANDARD, signature_b64)
        .map_err(|e| format!("署名base64デコード失敗: {}", e))?;

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
        Err(e) => Err(format!("署名検証失敗: {:?}", e)),
    }
}

/// Signature ヘッダーを簡易パースする
fn parse_signature_header(header: &str) -> Result<HashMap<String, String>, String> {
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
) -> Result<String, String> {
    let mut lines = Vec::new();
    for header_name in header_list_str.split(' ') {
        let name_lower = header_name.to_lowercase();
        if name_lower == "(request-target)" {
            lines.push(format!("(request-target): {} {}", method.to_lowercase(), path));
        } else {
            let val = headers.get(&name_lower).ok_or_else(|| {
                format!("署名対象ヘッダー \"{}\" がリクエストに見つかりません", header_name)
            })?;
            lines.push(format!("{}: {}", name_lower, val));
        }
    }
    Ok(lines.join("\n"))
}

/// HTTP Signatures 付きで ActivityPub エンドポイントへ POST する
///
/// # 引数
/// - `url`: 送信先 URL（相手の inbox 等）
/// - `body`: JSON 文字列
/// - `actor_key_id`: 署名に使うキー ID（例: `https://beta.seiran.org/users/yubaj#main-key`）
/// - `private_key_pem`: RSA 秘密鍵 PEM
pub async fn sign_and_post(
    url: &str,
    body: &str,
    actor_key_id: &str,
    private_key_pem: &str,
) -> Result<(), String> {
    let now = chrono::Utc::now();
    let date_str = now.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

    let parsed_url = url::Url::parse(url)
        .map_err(|e| format!("URL パースエラー: {}", e))?;
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
        .map_err(|e| format!("RSA 秘密鍵パース失敗: {}", e))?;

    let mut hasher = Sha256::new();
    hasher.update(signing_string.as_bytes());
    let hashed = hasher.finalize();

    let sig_bytes = private_key
        .sign(Pkcs1v15Sign::new::<Sha256>(), &hashed)
        .map_err(|e| format!("RSA 署名失敗: {}", e))?;

    let sig_b64 = base64::Engine::encode(&base64::prelude::BASE64_STANDARD, sig_bytes);

    let signature_header = format!(
        r#"keyId="{}",algorithm="rsa-sha256",headers="(request-target) host date content-type digest",signature="{}""#,
        actor_key_id, sig_b64
    );

    let client = reqwest::Client::builder()
        .user_agent("seiran-federation/0.1.0")
        .build()
        .map_err(|e| format!("HTTP クライアント初期化失敗: {}", e))?;

    let res = client
        .post(url)
        .header("Date", &date_str)
        .header("Host", &host)
        .header("Content-Type", "application/activity+json")
        .header("Digest", &digest)
        .header("Signature", &signature_header)
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("POST リクエスト失敗: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body_text = res.text().await.unwrap_or_default();
        return Err(format!("POST レスポンスエラー {}: {}", status, body_text));
    }

    Ok(())
}
