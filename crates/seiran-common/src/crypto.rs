//! AES-256-GCM 対称暗号ユーティリティ。
//! DB に格納する機密フィールド（storage_providers.secret_key 等）の暗号化に使用する。
//!
//! 格納フォーマット: `base64( nonce(12B) || ciphertext || tag(16B) )`

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    aead::rand_core::RngCore,
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("暗号化失敗: {0}")]
    Encrypt(String),
    #[error("復号失敗: {0}")]
    Decrypt(String),
    #[error("base64 デコード失敗: {0}")]
    Decode(String),
    #[error("暗号化データが不正です")]
    InvalidData,
    #[error("鍵長が不正です（32バイト必要）")]
    InvalidKeyLength,
}

/// `plaintext` を AES-256-GCM で暗号化し、base64 エンコードした文字列を返す。
/// `key` は 32バイトの生バイト列。
pub fn encrypt(plaintext: &[u8], key: &[u8]) -> Result<String, CryptoError> {
    if key.len() != 32 {
        return Err(CryptoError::InvalidKeyLength);
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| CryptoError::Encrypt(e.to_string()))?;

    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(STANDARD.encode(&combined))
}

/// `encoded`（base64 の `nonce || ciphertext || tag`）を復号して平文バイト列を返す。
pub fn decrypt(encoded: &str, key: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if key.len() != 32 {
        return Err(CryptoError::InvalidKeyLength);
    }
    let combined = STANDARD
        .decode(encoded)
        .map_err(|e| CryptoError::Decode(e.to_string()))?;
    if combined.len() < 12 {
        return Err(CryptoError::InvalidData);
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CryptoError::Decrypt(e.to_string()))
}
