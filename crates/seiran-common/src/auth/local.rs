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
    /// 自社ログイン発行分は 7 日で失効させるため Some。MiAuth（#60 アプリトークン）
    /// 発行分は明示的な無効化（`app_tokens.revoked_at`）まで有効であるべきトークンなので
    /// None（`exp` クレーム自体を持たせない）。Misskey 互換クライアント（Aria 等）は
    /// 「連携したら明示的に取り消すまで有効」という前提で作られており、両者を同じ
    /// 7日失効にすると再連携なしに突然 401 になる（実機で確認済みの不具合）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    exp: Option<usize>,
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
    /// トークンの `exp` クレーム（UNIX秒）。MiAuth 発行分は `None`。
    /// `verify_token_ignoring_exp` で検証した場合、値が過去でも呼び出し元が
    /// 別途 `app_tokens` を見て失効判定する前提でここに残す。
    pub exp: Option<usize>,
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
    /// 自社ログイン用に 7 日で失効するトークンを発行する。
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
        self.generate_token_with_exp(user_id, email, Some(exp))
    }

    /// MiAuth（#60: アプリトークン管理）発行用。`exp` クレームを持たせず、
    /// `app_tokens.revoked_at` による明示的な無効化まで有効なトークンを発行する。
    pub fn generate_app_token(
        &self,
        user_id: i64,
        email: &str,
    ) -> Result<(String, uuid::Uuid), AuthError> {
        self.generate_token_with_exp(user_id, email, None)
    }

    fn generate_token_with_exp(
        &self,
        user_id: i64,
        email: &str,
        exp: Option<usize>,
    ) -> Result<(String, uuid::Uuid), AuthError> {
        let now = chrono::Utc::now();
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
        self.verify_token_internal(token, true)
    }

    /// `exp` クレームを検証しない（署名検証・`sub`/`jti` 等の他クレームはそのまま検証する）
    /// バージョン。MiAuth（#60）発行トークンは無期限だが、この仕組み導入前に発行済みの
    /// 個体は `exp`（自社ログインと同じ7日）が埋め込まれたまま署名済みで、値を書き換える
    /// ことはできない。`extract_auth` 側でこの関数を使ってデコードした上で、`jti` が
    /// `app_tokens` に登録された有効な管理対象トークンであれば `exp` 切れを許容する
    /// （登録が無い＝自社ログイン等の管理対象外トークンは、呼び出し元が `exp` を
    /// 別途チェックして拒否する）。
    pub fn verify_token_ignoring_exp(&self, token: &str) -> Result<VerifiedUser, AuthError> {
        self.verify_token_internal(token, false)
    }

    fn verify_token_internal(&self, token: &str, validate_exp: bool) -> Result<VerifiedUser, AuthError> {
        let key = DecodingKey::from_secret(&self.secret);
        // MiAuth（#60）発行分は `exp` クレーム自体を持たない（無期限、`app_tokens.revoked_at`
        // で管理）ため、`exp` を必須クレームから外す。`exp` が存在するトークン
        // （自社ログイン発行分）については、jsonwebtoken が claims 中の `exp` を
        // 検出して自動的に期限切れを検証する（`validate_exp` のデフォルトは true）。
        let mut validation = Validation::default();
        validation.required_spec_claims.clear();
        validation.validate_exp = validate_exp;

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
            exp: token_data.claims.exp,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_token_has_exp_and_verifies() {
        let auth = LocalAuthProvider::new(b"test-secret".to_vec());
        let (token, jti) = auth.generate_token(1, "a@example.com").unwrap();
        let verified = auth.verify_token(&token).unwrap();
        assert_eq!(verified.user_id, 1);
        assert_eq!(verified.jti, jti);
    }

    /// MiAuth（#60）発行分は `exp` を持たないため、7日を過ぎても検証が通り続ける
    /// （再連携なしに突然 401 になる不具合の修正対象）。
    #[test]
    fn generate_app_token_has_no_exp_and_verifies() {
        let auth = LocalAuthProvider::new(b"test-secret".to_vec());
        let (token, jti) = auth.generate_app_token(1, "a@example.com").unwrap();
        let verified = auth.verify_token(&token).unwrap();
        assert_eq!(verified.user_id, 1);
        assert_eq!(verified.jti, jti);
    }

    #[test]
    fn expired_token_is_rejected() {
        let secret = b"test-secret".to_vec();
        let auth = LocalAuthProvider::new(secret.clone());
        let past_exp = (chrono::Utc::now() - chrono::Duration::days(1)).timestamp() as usize;
        let claims = LocalClaims {
            sub: "local|1".to_string(),
            email: "a@example.com".to_string(),
            exp: Some(past_exp),
            iat: chrono::Utc::now().timestamp() as usize,
            jti: uuid::Uuid::new_v4(),
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&secret),
        )
        .unwrap();
        assert!(auth.verify_token(&token).is_err());
    }

    /// この仕組み導入前に発行された MiAuth トークンは `exp`（自社ログインと同じ7日）が
    /// 埋め込まれたまま署名済みのため書き換えられない。`extract_auth` 側は
    /// `verify_token_ignoring_exp` でデコードした上で `app_tokens` の登録状況を見て
    /// 救済する（このテストは exp 無視デコード自体が過去の exp でも成功することを保証する）。
    #[test]
    fn verify_token_ignoring_exp_accepts_expired_token() {
        let secret = b"test-secret".to_vec();
        let auth = LocalAuthProvider::new(secret.clone());
        let past_exp = (chrono::Utc::now() - chrono::Duration::days(1)).timestamp() as usize;
        let jti = uuid::Uuid::new_v4();
        let claims = LocalClaims {
            sub: "local|1".to_string(),
            email: "a@example.com".to_string(),
            exp: Some(past_exp),
            iat: chrono::Utc::now().timestamp() as usize,
            jti,
        };
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&secret),
        )
        .unwrap();

        assert!(auth.verify_token(&token).is_err());
        let verified = auth.verify_token_ignoring_exp(&token).unwrap();
        assert_eq!(verified.jti, jti);
        assert_eq!(verified.exp, Some(past_exp));
    }
}
