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
    /// 現在発行するトークンは自社ログイン・MiAuth（#60 アプリトークン）とも `exp` クレーム
    /// 自体を持たせず無期限にする（失効は明示的な無効化 `app_tokens.revoked_at`／
    /// パスワード変更等による一括失効 `token_valid_after` のみで管理）。
    /// この仕組み導入前に発行された既存トークンには `exp`（7日）が署名済みで埋め込まれた
    /// ままのため書き換えられないが、検証時（`verify_token_ignoring_exp`）は一律で
    /// `exp` を無視するため、古いトークンも失効せず有効なまま扱われる。
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

/// AT Protocol セッション（`com.atproto.server.createSession`等）用JWT。
/// `LocalClaims`（`sub: "local|{user_id}"`）とは `sub` の形式が異なる（`sub` はDIDそのもの）
/// ため、既存の `verify_token`/`verify_token_ignoring_exp` には誤ってデコードされない。
#[derive(Debug, Serialize, Deserialize)]
struct AtpSessionClaims {
    /// "com.atproto.access" または "com.atproto.refresh"。
    scope: String,
    /// アカウントのDID。
    sub: String,
    /// PDSのサービスDID（`did:web:{local_domain}`）。
    aud: String,
    iat: usize,
    exp: usize,
    /// refreshJwt の失効・ローテーション管理用（`atp_refresh_tokens.jti`）。
    /// accessJwt にも便宜上同じ値を積むが、accessJwt側のjtiはDB管理しない。
    jti: uuid::Uuid,
}

pub struct VerifiedAtpAccess {
    pub did: String,
}

pub struct VerifiedAtpRefresh {
    pub did: String,
    pub jti: uuid::Uuid,
}

const ATP_ACCESS_SCOPE: &str = "com.atproto.access";
const ATP_REFRESH_SCOPE: &str = "com.atproto.refresh";

#[derive(Debug, Clone)]
pub struct VerifiedUser {
    pub user_id: i64,
    pub email: String,
    pub jti: uuid::Uuid,
    /// トークン発行時刻（UNIX秒）。
    pub iat: usize,
    /// トークンの `exp` クレーム（UNIX秒）。現在発行するトークンは持たないため`None`。
    /// この仕組み導入前に発行済みのトークンには埋め込まれたままの場合があるが、
    /// `verify_token_ignoring_exp` は検証に使わないため、呼び出し元も判定に使わない。
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

    /// `com.atproto.server.createAppPassword` 用のアプリパスワード生成（`xxxx-xxxx-xxxx-xxxx`
    /// 形式、Bluesky公式と同じ見た目）。
    pub fn generate_app_password() -> String {
        use argon2::password_hash::rand_core::{OsRng, RngCore};
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567890";
        let mut rng = OsRng;
        let mut group = || -> String {
            (0..4)
                .map(|_| CHARS[(rng.next_u32() as usize) % CHARS.len()] as char)
                .collect::<String>()
        };
        format!("{}-{}-{}-{}", group(), group(), group(), group())
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
    /// 自社ログイン用トークンを発行する。`exp` クレームは持たせず無期限とし、
    /// 明示的な無効化（設定画面の「全セッションからログアウト」等）まで有効とする。
    pub fn generate_token(
        &self,
        user_id: i64,
        email: &str,
    ) -> Result<(String, uuid::Uuid), AuthError> {
        self.generate_token_with_exp(user_id, email, None)
    }

    /// MiAuth（#60: アプリトークン管理）発行用。`app_tokens.revoked_at` による
    /// 明示的な無効化まで有効なトークンを発行する（`generate_token`と実質同じ挙動）。
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

    /// ログイントークンは無期限（`exp` クレーム自体を持たない、または検証時に無視する）
    /// ため、`exp` を検証しない。この仕組み導入前に発行済みのトークンには `exp`（旧仕様の
    /// 7日失効）が署名済みで埋め込まれたままだが、値を書き換えることはできないため、
    /// 検証時に無視することで無期限化を後方互換に成立させる。失効は明示的な無効化
    /// （`app_tokens.revoked_at`）・一括失効（`users.token_valid_after`）でのみ行う。
    pub fn verify_token_ignoring_exp(&self, token: &str) -> Result<VerifiedUser, AuthError> {
        let key = DecodingKey::from_secret(&self.secret);
        let mut validation = Validation::default();
        validation.required_spec_claims.clear();
        validation.validate_exp = false;

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

    /// `com.atproto.server.createSession`/`refreshSession` 用。accessJwt（2時間）と
    /// refreshJwt（90日）のペアを発行する。refreshJwt の `jti` は呼び出し側が
    /// `atp_refresh_tokens` に記録すること（失効・ローテーション管理用）。
    pub fn generate_atp_session(
        &self,
        did: &str,
        service_did: &str,
    ) -> Result<(String, String, uuid::Uuid, chrono::DateTime<chrono::Utc>), AuthError> {
        let now = chrono::Utc::now();
        let access_exp = now + chrono::Duration::hours(2);
        let refresh_exp = now + chrono::Duration::days(90);
        let jti = uuid::Uuid::new_v4();

        let access_claims = AtpSessionClaims {
            scope: ATP_ACCESS_SCOPE.to_string(),
            sub: did.to_string(),
            aud: service_did.to_string(),
            iat: now.timestamp() as usize,
            exp: access_exp.timestamp() as usize,
            jti,
        };
        let refresh_claims = AtpSessionClaims {
            scope: ATP_REFRESH_SCOPE.to_string(),
            sub: did.to_string(),
            aud: service_did.to_string(),
            iat: now.timestamp() as usize,
            exp: refresh_exp.timestamp() as usize,
            jti,
        };

        let key = EncodingKey::from_secret(&self.secret);
        let access_jwt = encode(&Header::default(), &access_claims, &key)
            .map_err(|e| AuthError::TokenGeneration(e.to_string()))?;
        let refresh_jwt = encode(&Header::default(), &refresh_claims, &key)
            .map_err(|e| AuthError::TokenGeneration(e.to_string()))?;

        Ok((access_jwt, refresh_jwt, jti, refresh_exp))
    }

    /// accessJwt を検証する（`scope`/`aud` 不一致は拒否）。
    pub fn verify_atp_access_token(
        &self,
        token: &str,
        service_did: &str,
    ) -> Result<VerifiedAtpAccess, AuthError> {
        let claims = self.decode_atp_claims(token, service_did)?;
        if claims.scope != ATP_ACCESS_SCOPE || claims.aud != service_did {
            return Err(AuthError::InvalidToken);
        }
        Ok(VerifiedAtpAccess { did: claims.sub })
    }

    /// refreshJwt を検証する（`scope`/`aud` 不一致は拒否）。有効性（失効・ローテーション）の
    /// 最終判定は呼び出し側が `jti` で `atp_refresh_tokens` を確認すること。
    pub fn verify_atp_refresh_token(
        &self,
        token: &str,
        service_did: &str,
    ) -> Result<VerifiedAtpRefresh, AuthError> {
        let claims = self.decode_atp_claims(token, service_did)?;
        if claims.scope != ATP_REFRESH_SCOPE || claims.aud != service_did {
            return Err(AuthError::InvalidToken);
        }
        Ok(VerifiedAtpRefresh {
            did: claims.sub,
            jti: claims.jti,
        })
    }

    /// `aud` クレームを検証するには `jsonwebtoken` 側で `set_audience` の明示呼び出しが必須
    /// （`Validation::default()` は `validate_aud: true` かつ `aud: None` のため、クレーム側に
    /// `aud` が存在するだけで `InvalidAudience` として一律拒否してしまう）。
    fn decode_atp_claims(
        &self,
        token: &str,
        service_did: &str,
    ) -> Result<AtpSessionClaims, AuthError> {
        let key = DecodingKey::from_secret(&self.secret);
        let mut validation = Validation::default();
        validation.set_audience(&[service_did]);
        decode::<AtpSessionClaims>(token, &key, &validation)
            .map(|d| d.claims)
            .map_err(|_| AuthError::InvalidToken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_token_has_no_exp_and_verifies() {
        let auth = LocalAuthProvider::new(b"test-secret".to_vec());
        let (token, jti) = auth.generate_token(1, "a@example.com").unwrap();
        let verified = auth.verify_token_ignoring_exp(&token).unwrap();
        assert_eq!(verified.user_id, 1);
        assert_eq!(verified.jti, jti);
        assert_eq!(verified.exp, None);
    }

    #[test]
    fn generate_app_token_has_no_exp_and_verifies() {
        let auth = LocalAuthProvider::new(b"test-secret".to_vec());
        let (token, jti) = auth.generate_app_token(1, "a@example.com").unwrap();
        let verified = auth.verify_token_ignoring_exp(&token).unwrap();
        assert_eq!(verified.user_id, 1);
        assert_eq!(verified.jti, jti);
    }

    /// 無期限化（この変更）より前に発行されたトークンは `exp`（旧仕様の7日失効）が
    /// 埋め込まれたまま署名済みのため書き換えられない。`verify_token_ignoring_exp` は
    /// 過去の `exp` でも検証を通すことで、既存発行済みトークンを無期限として扱う。
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

        let verified = auth.verify_token_ignoring_exp(&token).unwrap();
        assert_eq!(verified.jti, jti);
        assert_eq!(verified.exp, Some(past_exp));
    }
}
