//! TOTP（RFC 6238）シークレット生成・otpauth URI組み立て・コード検証（#65）。
//!
//! DBには [`crate::crypto::encrypt`] で暗号化した base32 シークレットのみを保存する。
//! この層は平文シークレットを扱う短命な値としてのみ受け渡す（呼び出し元は保持しない）。

use aes_gcm::aead::{rand_core::RngCore, OsRng};
use totp_rs::{Algorithm, Secret, TOTP};

const ISSUER: &str = "seiran";

/// 新規シークレットを生成し、base32文字列として返す（`user_totp.secret_encrypted`へ
/// 暗号化して保存する前段）。
pub fn generate_secret_base32() -> String {
    match Secret::generate_secret() {
        Secret::Raw(bytes) => Secret::Raw(bytes).to_encoded().to_string(),
        Secret::Encoded(s) => s,
    }
}

fn build_totp(secret_base32: &str, account_name: &str) -> Result<TOTP, String> {
    let secret = Secret::Encoded(secret_base32.to_string())
        .to_bytes()
        .map_err(|e| format!("{:?}", e))?;
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret,
        Some(ISSUER.to_string()),
        account_name.to_string(),
    )
    .map_err(|e| e.to_string())
}

/// 認証アプリ登録用の `otpauth://` URI を組み立てる。
pub fn build_otpauth_url(secret_base32: &str, account_name: &str) -> Result<String, String> {
    let totp = build_totp(secret_base32, account_name)?;
    Ok(totp.get_url())
}

/// 入力コード（6桁数字）がシークレットに対して現在時刻±1ステップ（30秒）以内に有効か検証する。
pub fn verify_code(secret_base32: &str, account_name: &str, code: &str) -> bool {
    let Ok(totp) = build_totp(secret_base32, account_name) else {
        return false;
    };
    totp.check_current(code).unwrap_or(false)
}

/// リカバリーコードを1件生成する（`nnnn-nnnn`形式、書き写しやすい数字のみ）。
pub fn generate_recovery_code() -> String {
    let mut rng = OsRng;
    let n1 = rng.next_u32() % 10000;
    let n2 = rng.next_u32() % 10000;
    format!("{:04}-{:04}", n1, n2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_secret_builds_otpauth_url() {
        let secret = generate_secret_base32();
        let url = build_otpauth_url(&secret, "alice").unwrap();
        assert!(url.starts_with("otpauth://totp/"));
        assert!(url.contains("alice"));
        assert!(url.contains("issuer=seiran"));
    }

    #[test]
    fn recovery_code_has_expected_format() {
        let code = generate_recovery_code();
        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b'-');
        assert!(code
            .chars()
            .enumerate()
            .all(|(i, c)| i == 4 || c.is_ascii_digit()));
    }

    #[test]
    fn malformed_codes_are_rejected() {
        let secret = generate_secret_base32();
        assert!(!verify_code(&secret, "alice", "12345"));
        assert!(!verify_code("not-base32!", "alice", "123456"));
    }
}
