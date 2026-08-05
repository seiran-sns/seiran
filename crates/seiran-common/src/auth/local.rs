use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

/// 認証エラー
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("パスワードハッシュ失敗: {0}")]
    Hash(String),
    #[error("トークン生成失敗: {0}")]
    TokenGeneration(String),
    #[error("トークンが無効です")]
    InvalidToken,
    #[error("トークン形式が不正です")]
    MalformedToken,
}

#[derive(Debug, Serialize, Deserialize)]
struct LocalClaims {
    sub: String,
    email: String,
    exp: usize,
    /// 発行時刻（UNIX秒）。パスワード変更等による一括失効（`token_valid_after`）の
    /// 判定に使う。この機能追加前に発行された既存トークンには含まれないため、
    /// 欠落時は0（1970年）として扱い、`token_valid_after`が未設定のユーザーには
    /// 影響しないようにする（デプロイ時の強制全ログアウトを避けるための移行措置）。
    #[serde(default)]
    iat: usize,
    /// トークン個体の識別子（#60: アプリトークン管理）。MiAuth 発行分のみ
    /// `app_tokens` テーブルに記録され、無効化チェックに使われる。
    jti: uuid::Uuid,
}

/// TOTP（#65）: パスワード検証は済んだがTOTPコード検証はまだのユーザーに払い出す
/// 短命トークン。`LocalClaims`とはフィールド構成が異なるため
/// （`jti`必須 vs `purpose`必須）、`extract_auth`側の`verify_token`には
/// デコードが通らず、通常のAPI認証には使えない。
#[derive(Debug, Serialize, Deserialize)]
struct PendingTotpClaims {
    sub: String,
    exp: usize,
    /// 固定文字列 "totp_pending"。他の用途のトークンと取り違えないための保険。
    purpose: String,
}

#[derive(Debug, Clone)]
pub struct VerifiedUser {
    pub user_id: i64,
    pub email: String,
    pub jti: uuid::Uuid,
    /// トークン発行時刻（UNIX秒）。
    pub iat: usize,
}

pub struct LocalAuthProvider {
    secret: Vec<u8>,
}

impl LocalAuthProvider {
    pub fn new(secret: Vec<u8>) -> Self {
        Self { secret }
    }

    pub fn hash_password(password: &str) -> Result<String, AuthError> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| AuthError::Hash(e.to_string()))
    }

    pub fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
        let parsed_hash = PasswordHash::new(hash).map_err(|e| AuthError::Hash(e.to_string()))?;
        let argon2 = Argon2::default();
        Ok(argon2
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }

    /// ユーザーが存在しない/パスワード未設定の場合に検証時間を揃えるためのダミーハッシュ。
    /// 実在ユーザーとの応答時間差でアカウントの存在を判定できてしまうタイミング攻撃を防ぐ。
    pub fn dummy_hash() -> &'static str {
        static DUMMY_HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        DUMMY_HASH.get_or_init(|| {
            Self::hash_password("dummy-password-for-timing-safety")
                .expect("固定文字列のハッシュ化は失敗しない")
        })
    }

    /// 発行した JWT と、その `jti`（#60: アプリトークン管理での識別用）を返す。
    pub fn generate_token(
        &self,
        user_id: i64,
        email: &str,
    ) -> Result<(String, uuid::Uuid), AuthError> {
        let now = chrono::Utc::now();
        let exp = now
            .checked_add_signed(chrono::Duration::days(7))
            .unwrap()
            .timestamp() as usize;
        let jti = uuid::Uuid::new_v4();

        let claims = LocalClaims {
            sub: format!("local|{}", user_id),
            email: email.to_string(),
            exp,
            iat: now.timestamp() as usize,
            jti,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|e| AuthError::TokenGeneration(e.to_string()))?;
        Ok((token, jti))
    }

    pub fn verify_token(&self, token: &str) -> Result<VerifiedUser, AuthError> {
        let key = DecodingKey::from_secret(&self.secret);
        let validation = Validation::default();

        let token_data =
            decode::<LocalClaims>(token, &key, &validation).map_err(|_| AuthError::InvalidToken)?;

        let user_id: i64 = token_data
            .claims
            .sub
            .strip_prefix("local|")
            .and_then(|s| s.parse().ok())
            .ok_or(AuthError::MalformedToken)?;

        Ok(VerifiedUser {
            user_id,
            email: token_data.claims.email,
            jti: token_data.claims.jti,
            iat: token_data.claims.iat,
        })
    }

    /// TOTP（#65）: パスワード検証成功後、TOTPコード検証待ちの間だけ有効な
    /// 短命トークン（5分）を発行する。
    pub fn generate_pending_totp_token(&self, user_id: i64) -> Result<String, AuthError> {
        let exp = chrono::Utc::now()
            .checked_add_signed(chrono::Duration::minutes(5))
            .unwrap()
            .timestamp() as usize;
        let claims = PendingTotpClaims {
            sub: format!("local|{}", user_id),
            exp,
            purpose: "totp_pending".to_string(),
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|e| AuthError::TokenGeneration(e.to_string()))
    }

    /// `generate_pending_totp_token` で発行したトークンを検証し、`user_id` を返す。
    pub fn verify_pending_totp_token(&self, token: &str) -> Result<i64, AuthError> {
        let key = DecodingKey::from_secret(&self.secret);
        let validation = Validation::default();

        let token_data = decode::<PendingTotpClaims>(token, &key, &validation)
            .map_err(|_| AuthError::InvalidToken)?;

        if token_data.claims.purpose != "totp_pending" {
            return Err(AuthError::InvalidToken);
        }

        token_data
            .claims
            .sub
            .strip_prefix("local|")
            .and_then(|s| s.parse().ok())
            .ok_or(AuthError::MalformedToken)
    }
}
