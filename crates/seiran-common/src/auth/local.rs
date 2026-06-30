use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

#[derive(Debug, Serialize, Deserialize)]
struct LocalClaims {
    sub: String,
    email: String,
    exp: usize,
}

#[derive(Debug, Clone)]
pub struct VerifiedUser {
    pub user_id: i64,
    pub email: String,
}

pub struct LocalAuthProvider {
    secret: Vec<u8>,
}

impl LocalAuthProvider {
    pub fn new(secret: Vec<u8>) -> Self {
        Self { secret }
    }

    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| e.to_string())
    }

    pub fn verify_password(password: &str, hash: &str) -> Result<bool, String> {
        let parsed_hash = PasswordHash::new(hash).map_err(|e| e.to_string())?;
        let argon2 = Argon2::default();
        Ok(argon2.verify_password(password.as_bytes(), &parsed_hash).is_ok())
    }

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

    pub fn verify_token(&self, token: &str) -> Result<VerifiedUser, String> {
        let key = DecodingKey::from_secret(&self.secret);
        let validation = Validation::default();

        let token_data = decode::<LocalClaims>(token, &key, &validation)
            .map_err(|_| "トークンが無効です".to_string())?;

        let user_id: i64 = token_data
            .claims
            .sub
            .strip_prefix("local|")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| "トークン形式が不正です".to_string())?;

        Ok(VerifiedUser { user_id, email: token_data.claims.email })
    }
}
