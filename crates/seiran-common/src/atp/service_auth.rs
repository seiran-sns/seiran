//! AT Protocol サービス間認証JWT【スパイク実装】
//!
//! `app.bsky.video.uploadVideo` のような、アカウントの代わりに別サービスを呼び出す際に
//! 必要な自己署名JWTを組み立てる。atproto.com の仕様（iss/aud/lxm/exp）に従い、
//! アカウントの `at_signing_key_pem`（P-256, PKCS8 PEM）でES256署名する。

use chrono::Utc;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
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
    encode(&Header::new(Algorithm::ES256), &claims, &key)
        .map_err(|e| ServiceAuthError::Sign(format!("署名失敗: {}", e)))
}
