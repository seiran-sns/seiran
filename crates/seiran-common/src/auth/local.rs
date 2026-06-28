use async_trait::async_trait;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use crate::traits::{AuthProvider, ExtUserInfo, AuthError};

#[derive(Debug, Serialize, Deserialize)]
struct LocalClaims {
    sub: String,
    email: String,
    exp: usize,
}

pub struct LocalAuthProvider {
    secret: Vec<u8>,
}

impl LocalAuthProvider {
    pub fn new(secret: Vec<u8>) -> Self {
        Self { secret }
    }

    /// パスワードをハッシュ化します（登録・更新用）
    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| e.to_string())
    }

    /// パスワードを検証します（ログイン用）
    pub fn verify_password(password: &str, hash: &str) -> Result<bool, String> {
        let parsed_hash = PasswordHash::new(hash).map_err(|e| e.to_string())?;
        let argon2 = Argon2::default();
        Ok(argon2.verify_password(password.as_bytes(), &parsed_hash).is_ok())
    }

    /// ローカルユーザー用のJWTトークンを発行します
    pub fn generate_token(&self, user_id: i64, email: &str) -> Result<String, String> {
        let exp = chrono::Utc::now()
            .checked_add_signed(chrono::Duration::days(7))
            .unwrap()
            .timestamp() as usize;

        let claims = LocalClaims {
            sub: format!("local|{}", user_id),
            email: email.to_string(),
            exp,
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|e| e.to_string())
    }
}

#[async_trait]
impl AuthProvider for LocalAuthProvider {
    async fn verify_token(&self, token: &str) -> Result<ExtUserInfo, AuthError> {
        // 開発用ダミートークン対応（"mock-{sub}-{email}" の形式）
        if token.starts_with("mock-") {
            let parts: Vec<&str> = token.split('-').collect();
            if parts.len() >= 3 {
                return Ok(ExtUserInfo {
                    sub: format!("local|{}", parts[1]),
                    email: parts[2].to_string(),
                });
            }
        }

        let key = DecodingKey::from_secret(&self.secret);
        let validation = Validation::default();
        
        let token_data = decode::<LocalClaims>(token, &key, &validation)
            .map_err(|_| AuthError::InvalidToken)?;
            
        Ok(ExtUserInfo {
            sub: token_data.claims.sub,
            email: token_data.claims.email,
        })
    }
}
