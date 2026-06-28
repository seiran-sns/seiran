use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use crate::traits::{AuthProvider, ExtUserInfo, AuthError};

#[derive(Debug, Serialize, Deserialize)]
struct Auth0Claims {
    sub: String,
    email: Option<String>,
}

pub struct Auth0Provider {
    pub audience: String,
    pub issuer: String,
}

impl Auth0Provider {
    pub fn new(audience: String, issuer: String) -> Self {
        Self { audience, issuer }
    }
}

#[async_trait]
impl AuthProvider for Auth0Provider {
    async fn verify_token(&self, token: &str) -> Result<ExtUserInfo, AuthError> {
        // 開発用ダミートークン対応（"mock-{sub}-{email}" の形式）
        if token.starts_with("mock-") {
            let parts: Vec<&str> = token.split('-').collect();
            if parts.len() >= 3 {
                return Ok(ExtUserInfo {
                    sub: format!("auth0|{}", parts[1]),
                    email: parts[2].to_string(),
                });
            }
        }

        // 署名検証を行わずにトークンの中身（Claims）を取り出します（開発時のモック／フォールバック用）
        // 本番環境ではJWKS等から取得した鍵で適切な署名検証を行う必要があります。
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 {
            return Err(AuthError::InvalidToken);
        }

        let payload_bytes = URL_SAFE_NO_PAD.decode(parts[1])
            .map_err(|e| AuthError::VerificationFailed(format!("Base64 decode failed: {}", e)))?;
            
        let claims: Auth0Claims = serde_json::from_slice(&payload_bytes)
            .map_err(|e| AuthError::VerificationFailed(format!("JSON parse failed: {}", e)))?;
            
        Ok(ExtUserInfo {
            sub: claims.sub,
            email: claims.email.unwrap_or_default(),
        })
    }
}
