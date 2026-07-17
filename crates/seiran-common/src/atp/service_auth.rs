//! AT Protocol サービス間認証JWT【スパイク実装】
//!
//! `app.bsky.video.uploadVideo` のような、アカウントの代わりに別サービスを呼び出す際に
//! 必要な自己署名JWTを組み立てる。atproto.com の仕様（iss/aud/lxm/exp）に従い、
//! アカウントの `at_signing_key_pem`（P-256, PKCS8 PEM）でES256署名する。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use p256::ecdsa::Signature;
use serde::Serialize;

#[derive(Serialize)]
struct ServiceAuthClaims {
    iss: String,
    aud: String,
    lxm: String,
    exp: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceAuthError {
    #[error("JWT署名エラー: {0}")]
    Sign(String),
}

/// `iss`（アカウントのDID）・`aud`（対象サービスのDID、`#fragment`込み可）・
/// `lxm`（呼び出すXRPCメソッド名）を指定してサービス間認証JWTを自己署名する。
/// 有効期限は仕様推奨の60秒。
pub fn sign_service_auth_jwt(pem: &str, iss: &str, aud: &str, lxm: &str) -> Result<String, ServiceAuthError> {
    let claims = ServiceAuthClaims {
        iss: iss.to_string(),
        aud: aud.to_string(),
        lxm: lxm.to_string(),
        exp: Utc::now().timestamp() + 60,
    };
    let key = EncodingKey::from_ec_pem(pem.as_bytes())
        .map_err(|e| ServiceAuthError::Sign(format!("鍵読み込み失敗: {}", e)))?;
    let jwt = encode(&Header::new(Algorithm::ES256), &claims, &key)
        .map_err(|e| ServiceAuthError::Sign(format!("署名失敗: {}", e)))?;
    normalize_jwt_es256_signature(&jwt)
}

/// `jsonwebtoken`（内部で `ring` を使用）の ES256 署名はデフォルトで low-S に正規化
/// されず、s 値は数学的性質上ほぼ50%の確率で high-S になる。AT Protocol のサービス間
/// 認証検証（`com.atproto.repo.uploadBlob` 等の呼び出し先）は
/// `crates/seiran-common/src/atp/repo.rs` の commit 署名と同じく low-S を要求するため、
/// JWT の署名セグメントだけを取り出して矯正し、組み直す。
fn normalize_jwt_es256_signature(jwt: &str) -> Result<String, ServiceAuthError> {
    let mut parts = jwt.rsplitn(2, '.');
    let sig_b64 = parts.next().ok_or_else(|| ServiceAuthError::Sign("JWT形式が不正".to_string()))?;
    let header_payload = parts.next().ok_or_else(|| ServiceAuthError::Sign("JWT形式が不正".to_string()))?;

    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|e| ServiceAuthError::Sign(format!("署名デコード失敗: {}", e)))?;
    let sig = Signature::try_from(sig_bytes.as_slice())
        .map_err(|e| ServiceAuthError::Sign(format!("署名パース失敗: {}", e)))?;
    let normalized = sig.normalize_s().unwrap_or(sig);
    let normalized_b64 = URL_SAFE_NO_PAD.encode(normalized.to_bytes());

    Ok(format!("{}.{}", header_payload, normalized_b64))
}
