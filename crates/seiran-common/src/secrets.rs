//! シークレット自動生成・永続化モジュール
//!
//! `secrets.toml` はユーザーが手動で作成するものではなく、
//! 起動時に設定ディレクトリ内に存在しなければ自動生成される。
//! ユーザーはこのファイルを含むディレクトリごと Docker ボリュームマウントすることで
//! シークレットを永続化・サルベージできる。
//!
//! # ファイル配置
//! ```
//! config/
//! ├── seiran.env      # ユーザーが編集する設定
//! └── secrets.toml    # 自動生成（触らなくてよい）
//! ```

use p256::ecdsa::SigningKey;
use p256::pkcs8::{
    EncodePrivateKey as P256EncodePrivateKey, EncodePublicKey as P256EncodePublicKey, LineEnding,
};
// p256 と argon2 は同じ rand_core v0.6.x を使うため、
// argon2 が再エクスポートする OsRng を利用してバージョン競合を回避する。
use argon2::password_hash::rand_core::OsRng;
use rsa::RsaPrivateKey;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 自動生成されるシークレット群
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secrets {
    /// JWT署名用シークレット（hex文字列、256bit）
    /// ローカル認証モードで JWT を発行・検証するために使用する。
    /// Auth0 モード時は使用しないが、将来のフォールバック用に常に生成する。
    pub jwt_secret: String,

    /// AT Protocol PDS 用 P-256 秘密鍵（PEM形式）
    /// `did:key` として公開され、ATP リポジトリへの署名に使用する。
    pub atproto_private_key_pem: String,

    /// AT Protocol PDS 用 P-256 公開鍵（PEM形式）
    pub atproto_public_key_pem: String,

    /// ActivityPub HTTP Signatures 用 RSA-2048 秘密鍵（PKCS#8 PEM形式）
    /// 既存の secrets.toml との後方互換性のため Option。
    #[serde(default)]
    pub ap_private_key_pem: Option<String>,

    /// ActivityPub HTTP Signatures 用 RSA-2048 公開鍵（PKCS#8 PEM形式）
    /// AP アクタードキュメントの publicKey として公開される。
    #[serde(default)]
    pub ap_public_key_pem: Option<String>,
}

impl Secrets {
    /// 新しいシークレットをランダムに生成する
    pub fn generate() -> Result<Self, SecretsError> {
        use argon2::password_hash::rand_core::RngCore;

        let mut rng = OsRng;

        // --- JWT secret: 256bit ランダム値 (hex) ---
        let mut jwt_bytes = [0u8; 32];
        rng.fill_bytes(&mut jwt_bytes);
        let jwt_secret = hex::encode(jwt_bytes);

        // --- AT Protocol P-256 鍵ペア ---
        let signing_key = SigningKey::random(&mut rng);
        let verifying_key = signing_key.verifying_key();

        let atproto_private_key_pem = signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| SecretsError::KeyGen(e.to_string()))?
            .to_string();

        let atproto_public_key_pem = verifying_key
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| SecretsError::KeyGen(e.to_string()))?;

        // --- ActivityPub HTTP Signatures 用 RSA-2048 鍵ペア ---
        let (ap_private_key_pem, ap_public_key_pem) = generate_rsa_key_pair(&mut rng)?;

        Ok(Self {
            jwt_secret,
            atproto_private_key_pem,
            atproto_public_key_pem,
            ap_private_key_pem: Some(ap_private_key_pem),
            ap_public_key_pem: Some(ap_public_key_pem),
        })
    }

    /// AP RSA 鍵が未設定の場合に生成して補完する（旧 secrets.toml の移行用）
    pub fn ensure_ap_keys(&mut self) -> Result<bool, SecretsError> {
        if self.ap_private_key_pem.is_some() && self.ap_public_key_pem.is_some() {
            return Ok(false);
        }
        let mut rng = OsRng;
        let (priv_pem, pub_pem) = generate_rsa_key_pair(&mut rng)?;
        self.ap_private_key_pem = Some(priv_pem);
        self.ap_public_key_pem = Some(pub_pem);
        eprintln!("[seiran] AP RSA 鍵ペアを新規生成しました。");
        Ok(true)
    }


    /// `jwt_secret` を生バイト列として返す（`LocalAuthProvider` に渡す用）
    pub fn jwt_secret_bytes(&self) -> Vec<u8> {
        hex::decode(&self.jwt_secret).unwrap_or_else(|_| self.jwt_secret.as_bytes().to_vec())
    }
}

/// シークレットファイルのパス管理
pub struct SecretsFile {
    path: PathBuf,
}

impl SecretsFile {
    /// 設定ディレクトリを受け取り、その中の `secrets.toml` を管理する
    pub fn new(config_dir: impl AsRef<Path>) -> Self {
        Self {
            path: config_dir.as_ref().join("secrets.toml"),
        }
    }

    /// `SEIRAN_CONFIG_DIR` 環境変数またはデフォルトパス (`./config`) から構築する
    pub fn from_env() -> Self {
        let dir = std::env::var("SEIRAN_CONFIG_DIR").unwrap_or_else(|_| "./config".to_string());
        Self::new(dir)
    }

    /// シークレットを読み込む。ファイルが存在しなければ自動生成して保存する。
    /// 既存ファイルに AP RSA 鍵が未設定の場合は自動補完する。
    pub fn load_or_create(&self) -> Result<Secrets, SecretsError> {
        if self.path.exists() {
            let mut secrets = self.load()?;
            // 旧 secrets.toml に AP 鍵が無い場合は補完して保存
            if secrets.ensure_ap_keys()? {
                self.save(&secrets)?;
            }
            Ok(secrets)
        } else {
            eprintln!(
                "[seiran] secrets.toml が見つかりません。新規生成します: {}",
                self.path.display()
            );
            let secrets = Secrets::generate()?;
            self.save(&secrets)?;
            eprintln!(
                "[seiran] secrets.toml を生成しました。このファイルを安全に保管してください。"
            );
            Ok(secrets)
        }
    }

    /// 既存の `secrets.toml` を読み込む
    fn load(&self) -> Result<Secrets, SecretsError> {
        let content = std::fs::read_to_string(&self.path)
            .map_err(|e| SecretsError::Io(e.to_string()))?;
        toml::from_str(&content).map_err(|e| SecretsError::Parse(e.to_string()))
    }

    /// シークレットを `secrets.toml` に書き出す
    fn save(&self, secrets: &Secrets) -> Result<(), SecretsError> {
        // 親ディレクトリを作成
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SecretsError::Io(e.to_string()))?;
        }

        let header = concat!(
            "# seiran - 自動生成シークレットファイル\n",
            "# このファイルはサーバー起動時に自動生成されます。\n",
            "# 手動で編集する必要はありません。\n",
            "# このファイルを含むディレクトリを Docker ボリュームにマウントして永続化してください。\n",
            "#\n",
            "# ⚠️  このファイルを Git にコミットしないでください。\n",
            "# ⚠️  このファイルを紛失すると既存の JWT トークンと ATP 署名が無効になります。\n\n"
        );

        let body = toml::to_string_pretty(secrets)
            .map_err(|e| SecretsError::Serialize(e.to_string()))?;

        std::fs::write(&self.path, format!("{}{}", header, body))
            .map_err(|e| SecretsError::Io(e.to_string()))?;

        // パーミッションを 600 に設定（所有者のみ読み書き可）
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.path, perms)
                .map_err(|e| SecretsError::Io(e.to_string()))?;
        }

        Ok(())
    }

    /// ファイルパスへの参照
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// シークレット操作エラー
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("I/O エラー: {0}")]
    Io(String),

    #[error("TOML パースエラー: {0}")]
    Parse(String),

    #[error("TOML シリアライズエラー: {0}")]
    Serialize(String),

    #[error("鍵ペア生成エラー: {0}")]
    KeyGen(String),
}

/// RSA-2048 鍵ペアを生成し (秘密鍵 PEM, 公開鍵 PEM) を返す
fn generate_rsa_key_pair(
    rng: &mut OsRng,
) -> Result<(String, String), SecretsError> {
    let private_key = RsaPrivateKey::new(rng, 2048)
        .map_err(|e| SecretsError::KeyGen(format!("RSA鍵生成失敗: {}", e)))?;
    let public_key = private_key.to_public_key();

    let private_pem = private_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| SecretsError::KeyGen(format!("RSA秘密鍵PEM変換失敗: {}", e)))?
        .to_string();

    let public_pem = public_key
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| SecretsError::KeyGen(format!("RSA公開鍵PEM変換失敗: {}", e)))?;

    Ok((private_pem, public_pem))
}

