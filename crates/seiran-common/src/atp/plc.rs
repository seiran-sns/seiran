use argon2::password_hash::rand_core::OsRng;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use p256::ecdsa::{signature::Signer, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum PlcError {
    #[error("鍵生成エラー: {0}")]
    KeyGen(String),
    #[error("CBOR エンコードエラー: {0}")]
    Cbor(String),
    #[error("HTTP エラー: {0}")]
    Http(String),
    #[error("plc.directory 登録失敗 (HTTP {status}): {body}")]
    PlcDirectory { status: u16, body: String },
}

/// P-256 公開鍵を `did:key` 形式に変換する
/// multicodec: p256-pub = 0x1200 → varint [0x80, 0x24]
pub fn p256_to_did_key(verifying_key: &p256::ecdsa::VerifyingKey) -> String {
    let compressed = verifying_key.to_encoded_point(true);
    let mut buf = vec![0x80u8, 0x24u8];
    buf.extend_from_slice(compressed.as_bytes());
    format!("did:key:z{}", bs58::encode(&buf).into_string())
}

/// PEM 文字列から P-256 SigningKey を復元する
pub fn signing_key_from_pem(pem: &str) -> Result<SigningKey, PlcError> {
    SigningKey::from_pkcs8_pem(pem).map_err(|e| PlcError::KeyGen(e.to_string()))
}

// ─── DAG-CBOR 用データ構造 ────────────────────────────────────────────────────
// serde_ipld_dagcbor はフィールド名を canonical 順（バイト長→辞書順）にソートする。
// struct の宣言順に関係なく CBOR 出力は仕様通りになる。

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlcService {
    endpoint: String,
    r#type: String,
}

/// 署名前オペレーション（sig なし）— CBOR エンコードして署名対象にする
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenesisOpUnsigned {
    also_known_as: Vec<String>,
    prev: Option<String>,
    rotation_keys: Vec<String>,
    services: BTreeMap<String, PlcService>,
    r#type: String,
    verification_methods: BTreeMap<String, String>,
}

/// 署名済みオペレーション（sig あり）— CBOR エンコードして DID を計算し、JSON で POST する
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenesisOpSigned {
    also_known_as: Vec<String>,
    prev: Option<String>,
    rotation_keys: Vec<String>,
    services: BTreeMap<String, PlcService>,
    sig: String,
    r#type: String,
    verification_methods: BTreeMap<String, String>,
}

// ─── 登録 ─────────────────────────────────────────────────────────────────────

/// genesis op 生成済みデータ。DID は確定しているが plc.directory にはまだ送信していない。
pub struct PlcGenesis {
    pub did: String,
    pub signing_key_pem: String,
    signed_op: GenesisOpSigned,
}

/// DID を確定させ genesis op を準備する（ネットワーク通信なし）。
/// plc.directory への送信は `submit_plc_genesis` で別途行う。
///
/// RFC 6979 による決定論的 ECDSA は user_signing_key が変わると署名も変わる。
/// base64url の `-` / `_` を含まない署名が得られるまで user_signing_key を引き直す。
pub fn prepare_plc_genesis(
    username: &str,
    pds_domain: &str,
    rotation_signing_key: &SigningKey,
) -> Result<PlcGenesis, PlcError> {
    let rotation_did_key = p256_to_did_key(rotation_signing_key.verifying_key());
    let handle = format!("at://{}.{}", username, pds_domain);
    let pds_endpoint = format!("https://{}", pds_domain);

    // user_signing_key を引き直すたびに署名が変わる。
    // `-` / `_` を含まない署名が出るまで最大 200 回試行する（平均 ~15 回で当たる）。
    for attempt in 0..200usize {
        let user_signing_key = SigningKey::random(&mut OsRng);
        let user_did_key = p256_to_did_key(user_signing_key.verifying_key());

        let mut verification_methods = BTreeMap::new();
        verification_methods.insert("atproto".to_string(), user_did_key);

        let mut services_unsigned = BTreeMap::new();
        services_unsigned.insert(
            "atproto_pds".to_string(),
            PlcService { endpoint: pds_endpoint.clone(), r#type: "AtprotoPersonalDataServer".to_string() },
        );

        // ① 署名前オペレーションを DAG-CBOR エンコード → rotation key で署名
        let unsigned_op = GenesisOpUnsigned {
            also_known_as: vec![handle.clone()],
            prev: None,
            rotation_keys: vec![rotation_did_key.clone()],
            services: services_unsigned,
            r#type: "plc_operation".to_string(),
            verification_methods: verification_methods.clone(),
        };
        let unsigned_cbor = serde_ipld_dagcbor::to_vec(&unsigned_op)
            .map_err(|e| PlcError::Cbor(e.to_string()))?;

        let raw_sig: p256::ecdsa::Signature = rotation_signing_key.sign(&unsigned_cbor);
        let sig_str = URL_SAFE_NO_PAD.encode(raw_sig.to_bytes().as_slice());

        // plc.directory が base64url の `-` / `_` を含む署名を拒否する場合があるため除外
        if sig_str.contains('-') || sig_str.contains('_') {
            if attempt < 5 {
                eprintln!("[plc] sig に -/_ 含まれるため引き直し (attempt={})", attempt + 1);
            }
            continue;
        }

        // ② 署名済みオペレーションを DAG-CBOR エンコード → SHA-256 → DID
        let mut services_signed = BTreeMap::new();
        services_signed.insert(
            "atproto_pds".to_string(),
            PlcService { endpoint: pds_endpoint.clone(), r#type: "AtprotoPersonalDataServer".to_string() },
        );
        let signed_op = GenesisOpSigned {
            also_known_as: vec![handle.clone()],
            prev: None,
            rotation_keys: vec![rotation_did_key.clone()],
            services: services_signed,
            sig: sig_str,
            r#type: "plc_operation".to_string(),
            verification_methods,
        };
        let signed_cbor = serde_ipld_dagcbor::to_vec(&signed_op)
            .map_err(|e| PlcError::Cbor(e.to_string()))?;
        let hash = Sha256::digest(&signed_cbor);
        let b32 = base32::encode(
            base32::Alphabet::RFC4648 { padding: false },
            hash.as_slice(),
        )
        .to_lowercase();
        let did = format!("did:plc:{}", &b32[..24]);

        let signing_key_pem = user_signing_key
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| PlcError::KeyGen(e.to_string()))?
            .to_string();

        eprintln!("[plc] sig 確定 (attempt={}): {}", attempt + 1, &signed_op.sig[..8]);
        return Ok(PlcGenesis { did, signing_key_pem, signed_op });
    }

    Err(PlcError::KeyGen("200 回試行しても -/_ なし署名が得られませんでした".to_string()))
}

/// 準備済み genesis op を plc.directory に送信する（ネットワーク通信あり）。
pub async fn submit_plc_genesis(
    genesis: &PlcGenesis,
    client: &reqwest::Client,
) -> Result<(), PlcError> {
    let url = format!("https://plc.directory/{}", genesis.did);
    let res = client
        .post(&url)
        .json(&genesis.signed_op)
        .send()
        .await
        .map_err(|e| PlcError::Http(e.to_string()))?;

    let status = res.status().as_u16();
    if status != 200 && status != 201 {
        let body = res.text().await.unwrap_or_default();
        return Err(PlcError::PlcDirectory { status, body });
    }

    Ok(())
}
