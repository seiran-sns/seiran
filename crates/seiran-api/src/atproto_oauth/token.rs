//! OAuth のアクセストークンとリフレッシュトークン。アクセストークンはサーバーの JWT 秘密鍵で
//! HS256 署名した JWT で、クライアントにとっては中身を読まない不透明な値。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// アクセストークンの有効期間。
pub const ACCESS_TOKEN_TTL_SECS: i64 = 3600;
/// ヘッダーの `typ`。`createSession` のトークンや seiran のログイントークンと取り違えないための印。
const ACCESS_TOKEN_TYP: &str = "at+jwt";

#[derive(Debug, Serialize, Deserialize)]
struct Confirmation {
    jkt: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AccessClaims {
    iss: String,
    aud: String,
    sub: String,
    scope: String,
    sid: String,
    cnf: Confirmation,
    iat: i64,
    exp: i64,
}

/// 検証済みのアクセストークン。セッションがまだ有効かは呼び出し側が DB で確かめる。
#[derive(Debug)]
pub struct VerifiedAccessToken {
    pub did: String,
    pub session_id: i64,
    pub jkt: String,
}

pub struct NewAccessToken<'a> {
    pub issuer: &'a str,
    pub did: &'a str,
    pub scope: &'a str,
    pub session_id: i64,
    pub jkt: &'a str,
}

pub fn issue_access_token(secret: &[u8], t: &NewAccessToken<'_>) -> Result<String, String> {
    let now = chrono::Utc::now().timestamp();
    let mut header = Header::new(Algorithm::HS256);
    header.typ = Some(ACCESS_TOKEN_TYP.to_string());
    let claims = AccessClaims {
        iss: t.issuer.to_string(),
        aud: t.issuer.to_string(),
        sub: t.did.to_string(),
        scope: t.scope.to_string(),
        sid: t.session_id.to_string(),
        cnf: Confirmation {
            jkt: t.jkt.to_string(),
        },
        iat: now,
        exp: now + ACCESS_TOKEN_TTL_SECS,
    };
    jsonwebtoken::encode(&header, &claims, &EncodingKey::from_secret(secret))
        .map_err(|e| e.to_string())
}

pub fn verify_access_token(
    secret: &[u8],
    issuer: &str,
    token: &str,
) -> Option<VerifiedAccessToken> {
    let header = jsonwebtoken::decode_header(token).ok()?;
    if header.typ.as_deref() != Some(ACCESS_TOKEN_TYP) {
        return None;
    }
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_audience(&[issuer]);
    validation.set_issuer(&[issuer]);
    let claims =
        jsonwebtoken::decode::<AccessClaims>(token, &DecodingKey::from_secret(secret), &validation)
            .ok()?
            .claims;
    Some(VerifiedAccessToken {
        did: claims.sub,
        session_id: claims.sid.parse().ok()?,
        jkt: claims.cnf.jkt,
    })
}

/// リフレッシュトークン・認可コード・`request_uri` に使うランダム値。
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS の乱数を取得できません");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// DB にはリフレッシュトークン・認可コードのハッシュだけを置く。
pub fn hash_secret(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUER: &str = "https://example.com";

    fn token() -> String {
        issue_access_token(
            b"secret",
            &NewAccessToken {
                issuer: ISSUER,
                did: "did:plc:abc",
                scope: "atproto transition:generic",
                session_id: 42,
                jkt: "thumb",
            },
        )
        .unwrap()
    }

    #[test]
    fn round_trips() {
        let v = verify_access_token(b"secret", ISSUER, &token()).unwrap();
        assert_eq!(v.did, "did:plc:abc");
        assert_eq!(v.session_id, 42);
        assert_eq!(v.jkt, "thumb");
    }

    #[test]
    fn rejects_other_secret_or_issuer() {
        assert!(verify_access_token(b"other", ISSUER, &token()).is_none());
        assert!(verify_access_token(b"secret", "https://other.example", &token()).is_none());
    }

    #[test]
    fn rejects_tokens_without_access_token_typ() {
        // 同じ秘密鍵で署名された別種の JWT（typ が JWT）は受け付けない。
        let claims = serde_json::json!({
            "iss": ISSUER, "aud": ISSUER, "sub": "did:plc:abc", "scope": "atproto",
            "sid": "1", "cnf": {"jkt": "t"}, "iat": 0, "exp": 9_999_999_999i64
        });
        let other = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
        assert!(verify_access_token(b"secret", ISSUER, &other).is_none());
    }
}
