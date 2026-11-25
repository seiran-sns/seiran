//! DPoP（RFC 9449）の証明の検証と nonce。`docs/protocols.md` 3節「OAuth」の DPoP。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// 証明の `iat` として受け付ける、サーバー時刻とのずれ。
const MAX_CLOCK_SKEW_SECS: i64 = 300;
/// nonce を作り替える間隔。現在と直前の刻みを受け付けるので、1つの nonce は最長でこの2倍使える。
const NONCE_PERIOD_SECS: i64 = 180;
/// `jti` を覚えておく時間。nonce が受け付けられる最長の時間と、`iat` のずれを合わせた長さ。
pub const JTI_TTL_SECS: u64 = (NONCE_PERIOD_SECS * 2 + MAX_CLOCK_SKEW_SECS) as u64;

#[derive(Debug, PartialEq, Eq)]
pub enum DpopError {
    /// nonce が無い・古い。クライアントは新しい nonce で送り直せばよい。
    UseNonce,
    Invalid(&'static str),
}

/// 検証済みの証明。
#[derive(Debug)]
pub struct DpopProof {
    /// 証明に使われた公開鍵の拇印（RFC 7638）。
    pub jkt: String,
    pub jti: String,
}

#[derive(Deserialize)]
struct ProofHeader {
    typ: Option<String>,
    alg: Option<String>,
    jwk: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ProofClaims {
    jti: Option<String>,
    htm: Option<String>,
    htu: Option<String>,
    iat: Option<i64>,
    nonce: Option<String>,
    ath: Option<String>,
}

/// nonce は保存せず、サーバーの秘密と時刻の刻みから作る（プロセスが複数あっても共有が要らない）。
pub struct NonceSource {
    key: [u8; 32],
}

impl NonceSource {
    /// `secret` はサーバーの JWT 秘密鍵。そのまま使わず、用途を混ぜて別の鍵にする。
    pub fn new(secret: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"seiran-atproto-dpop-nonce:");
        hasher.update(secret);
        Self {
            key: hasher.finalize().into(),
        }
    }

    fn for_bucket(&self, bucket: i64) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.key);
        hasher.update(bucket.to_be_bytes());
        URL_SAFE_NO_PAD.encode(hasher.finalize())
    }

    fn bucket(now: i64) -> i64 {
        now.div_euclid(NONCE_PERIOD_SECS)
    }

    pub fn current(&self) -> String {
        self.for_bucket(Self::bucket(chrono::Utc::now().timestamp()))
    }

    fn is_valid_at(&self, nonce: &str, now: i64) -> bool {
        let b = Self::bucket(now);
        nonce == self.for_bucket(b) || nonce == self.for_bucket(b - 1)
    }
}

/// RFC 7638 の拇印。EC 鍵は `crv`・`kty`・`x`・`y` をこの順に並べた JSON のハッシュ。
pub fn ec_thumbprint(x: &str, y: &str) -> String {
    let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
    URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
}

/// アクセストークンの SHA-256（証明の `ath` と比べる値）。
pub fn access_token_hash(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

/// `htu` の比較用に、クエリとフラグメントを落とす。
fn strip_query(url: &str) -> &str {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    &url[..end]
}

/// 証明を検証する。`htu` は期待する URL（クエリ無し）、`access_token` は XRPC で照合する
/// アクセストークン。再利用（`jti`）の確認は呼び出し側が行う。
pub fn verify(
    proof: &str,
    nonces: &NonceSource,
    htm: &str,
    htu: &str,
    access_token: Option<&str>,
) -> Result<DpopProof, DpopError> {
    verify_at(
        proof,
        nonces,
        htm,
        htu,
        access_token,
        chrono::Utc::now().timestamp(),
    )
}

fn verify_at(
    proof: &str,
    nonces: &NonceSource,
    htm: &str,
    htu: &str,
    access_token: Option<&str>,
    now: i64,
) -> Result<DpopProof, DpopError> {
    let header_b64 = proof
        .split('.')
        .next()
        .ok_or(DpopError::Invalid("malformed proof"))?;
    let header: ProofHeader = URL_SAFE_NO_PAD
        .decode(header_b64)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or(DpopError::Invalid("malformed proof header"))?;
    if header.typ.as_deref() != Some("dpop+jwt") {
        return Err(DpopError::Invalid("typ must be dpop+jwt"));
    }
    if header.alg.as_deref() != Some("ES256") {
        return Err(DpopError::Invalid("alg must be ES256"));
    }
    let jwk = header.jwk.ok_or(DpopError::Invalid("missing jwk"))?;
    let field = |name: &str| jwk.get(name).and_then(|v| v.as_str());
    if field("kty") != Some("EC") || field("crv") != Some("P-256") {
        return Err(DpopError::Invalid("jwk must be a P-256 EC key"));
    }
    if jwk.get("d").is_some() {
        return Err(DpopError::Invalid("jwk must not contain a private key"));
    }
    let (Some(x), Some(y)) = (field("x"), field("y")) else {
        return Err(DpopError::Invalid("jwk must have x and y"));
    };
    let key =
        DecodingKey::from_ec_components(x, y).map_err(|_| DpopError::Invalid("invalid jwk"))?;

    let mut validation = Validation::new(Algorithm::ES256);
    validation.required_spec_claims.clear();
    validation.validate_exp = false;
    validation.validate_aud = false;
    let claims = jsonwebtoken::decode::<ProofClaims>(proof, &key, &validation)
        .map_err(|_| DpopError::Invalid("invalid proof signature"))?
        .claims;

    let jti = claims
        .jti
        .filter(|j| !j.is_empty() && j.len() <= 256)
        .ok_or(DpopError::Invalid("missing jti"))?;
    if claims.htm.as_deref() != Some(htm) {
        return Err(DpopError::Invalid("htm mismatch"));
    }
    match claims.htu.as_deref() {
        Some(u) if strip_query(u) == htu => {}
        _ => return Err(DpopError::Invalid("htu mismatch")),
    }
    let iat = claims.iat.ok_or(DpopError::Invalid("missing iat"))?;
    if (now - iat).abs() > MAX_CLOCK_SKEW_SECS {
        return Err(DpopError::Invalid("iat out of range"));
    }
    if let Some(token) = access_token {
        if claims.ath.as_deref() != Some(access_token_hash(token).as_str()) {
            return Err(DpopError::Invalid("ath mismatch"));
        }
    }
    // nonce は署名・鍵の検証が通ってから見る（無効な証明に新しい nonce を配っても意味が無いが、
    // 有効な証明なら nonce だけ直して送り直せば通るとクライアントに伝えたい）。
    match claims.nonce.as_deref() {
        Some(n) if nonces.is_valid_at(n, now) => {}
        _ => return Err(DpopError::UseNonce),
    }

    Ok(DpopProof {
        jkt: ec_thumbprint(x, y),
        jti,
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header};
    use p256::ecdsa::SigningKey;
    use p256::pkcs8::EncodePrivateKey;

    pub struct TestKey {
        pub encoding: EncodingKey,
        pub x: String,
        pub y: String,
    }

    pub fn new_key() -> TestKey {
        let signing = <SigningKey as p256::elliptic_curve::Generate>::generate();
        // 非圧縮の SEC1 表現は 0x04 || x(32) || y(32)。
        let point = signing.verifying_key().to_sec1_point(false);
        let bytes = point.as_bytes();
        let der = signing.to_pkcs8_der().unwrap();
        TestKey {
            encoding: EncodingKey::from_ec_der(der.as_bytes()),
            x: URL_SAFE_NO_PAD.encode(&bytes[1..33]),
            y: URL_SAFE_NO_PAD.encode(&bytes[33..65]),
        }
    }

    pub fn proof(key: &TestKey, claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("dpop+jwt".to_string());
        header.jwk = Some(
            serde_json::from_value(serde_json::json!({
                "kty": "EC", "crv": "P-256", "x": key.x, "y": key.y
            }))
            .unwrap(),
        );
        jsonwebtoken::encode(&header, &claims, &key.encoding).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    const HTU: &str = "https://example.com/oauth/atproto/token";
    const NOW: i64 = 1_800_000_000;

    fn nonces() -> NonceSource {
        NonceSource::new(b"secret")
    }

    fn claims(nonce: &str) -> serde_json::Value {
        serde_json::json!({"jti": "j1", "htm": "POST", "htu": HTU, "iat": NOW, "nonce": nonce})
    }

    #[test]
    fn accepts_valid_proof_and_returns_thumbprint() {
        let key = new_key();
        let n = nonces();
        let nonce = n.for_bucket(NonceSource::bucket(NOW));
        let proof = proof(&key, claims(&nonce));
        let ok = verify_at(&proof, &n, "POST", HTU, None, NOW).unwrap();
        assert_eq!(ok.jkt, ec_thumbprint(&key.x, &key.y));
        assert_eq!(ok.jti, "j1");
    }

    #[test]
    fn previous_nonce_is_accepted_but_older_is_not() {
        let key = new_key();
        let n = nonces();
        let b = NonceSource::bucket(NOW);
        let prev = proof(&key, claims(&n.for_bucket(b - 1)));
        assert!(verify_at(&prev, &n, "POST", HTU, None, NOW).is_ok());
        let old = proof(&key, claims(&n.for_bucket(b - 2)));
        assert_eq!(
            verify_at(&old, &n, "POST", HTU, None, NOW).unwrap_err(),
            DpopError::UseNonce
        );
    }

    #[test]
    fn missing_nonce_asks_for_one() {
        let key = new_key();
        let proof = proof(
            &key,
            serde_json::json!({"jti": "j1", "htm": "POST", "htu": HTU, "iat": NOW}),
        );
        assert_eq!(
            verify_at(&proof, &nonces(), "POST", HTU, None, NOW).unwrap_err(),
            DpopError::UseNonce
        );
    }

    #[test]
    fn rejects_wrong_method_url_or_access_token() {
        let key = new_key();
        let n = nonces();
        let nonce = n.for_bucket(NonceSource::bucket(NOW));
        let p = proof(&key, claims(&nonce));
        assert!(verify_at(&p, &n, "GET", HTU, None, NOW).is_err());
        assert!(verify_at(&p, &n, "POST", "https://example.com/other", None, NOW).is_err());
        assert!(verify_at(&p, &n, "POST", HTU, Some("token"), NOW).is_err());

        let mut with_ath = claims(&nonce);
        with_ath["ath"] = serde_json::json!(access_token_hash("token"));
        let p = proof(&key, with_ath);
        assert!(verify_at(&p, &n, "POST", HTU, Some("token"), NOW).is_ok());
    }

    #[test]
    fn htu_query_is_ignored() {
        let key = new_key();
        let n = nonces();
        let mut c = claims(&n.for_bucket(NonceSource::bucket(NOW)));
        c["htu"] = serde_json::json!(format!("{HTU}?a=b"));
        assert!(verify_at(&proof(&key, c), &n, "POST", HTU, None, NOW).is_ok());
    }

    #[test]
    fn rejects_stale_iat_and_foreign_signature() {
        let key = new_key();
        let n = nonces();
        let nonce = n.for_bucket(NonceSource::bucket(NOW));
        let mut c = claims(&nonce);
        c["iat"] = serde_json::json!(NOW - 3600);
        assert!(verify_at(&proof(&key, c), &n, "POST", HTU, None, NOW).is_err());

        // ヘッダーの鍵と違う鍵で署名した証明
        let other = new_key();
        let forged = proof(
            &TestKey {
                encoding: other.encoding,
                x: key.x.clone(),
                y: key.y.clone(),
            },
            claims(&nonce),
        );
        assert!(verify_at(&forged, &n, "POST", HTU, None, NOW).is_err());
    }
}
