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

/// リポジトリ署名用の新規P-256鍵ペアを生成する（PEM化込み）。
/// `prepare_plc_genesis`のuser_signing_key生成と同じロジックだが、既存DIDの
/// `verificationMethod`差し替え（アカウント転入フロー）でも使うため単体関数として切り出す。
pub fn generate_new_signing_key() -> Result<(SigningKey, String), PlcError> {
    let key = SigningKey::random(&mut OsRng);
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| PlcError::KeyGen(e.to_string()))?
        .to_string();
    Ok((key, pem))
}

// ─── DAG-CBOR 用データ構造 ────────────────────────────────────────────────────
// serde_ipld_dagcbor はフィールド名を canonical 順（バイト長→辞書順）にソートする。
// struct の宣言順に関係なく CBOR 出力は仕様通りになる。

#[derive(Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PlcService {
    endpoint: String,
    r#type: String,
}

/// 署名前の更新オペレーション（`prev`あり、genesisと違いDIDは既に確定済み）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateOpUnsigned {
    also_known_as: Vec<String>,
    prev: String,
    rotation_keys: Vec<String>,
    services: BTreeMap<String, PlcService>,
    r#type: String,
    verification_methods: BTreeMap<String, String>,
}

/// 署名済みの更新オペレーション。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateOpSigned {
    also_known_as: Vec<String>,
    prev: String,
    rotation_keys: Vec<String>,
    services: BTreeMap<String, PlcService>,
    sig: String,
    r#type: String,
    verification_methods: BTreeMap<String, String>,
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

/// plc.directory のベース URL。未設定時は本番の plc.directory。
/// E2E テストではローカルのスタブサーバーに向けるために使う。
pub fn plc_directory_base_url() -> String {
    std::env::var("PLC_DIRECTORY_BASE_URL")
        .unwrap_or_else(|_| "https://plc.directory".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// genesis op 生成済みデータ。DID は確定しているが plc.directory にはまだ送信していない。
pub struct PlcGenesis {
    pub did: String,
    pub signing_key_pem: String,
    /// アカウント単位で新規生成したローテーションキー（`rotationKeys[0]`＝主）。
    /// `server_shared_rotation_key`はrecovery用として`rotationKeys[1]`に副として残す
    /// （転出元API対応の前提、bsky.social本家のuser key + service recovery keyと同型）。
    pub rotation_key_pem: String,
    signed_op: GenesisOpSigned,
}

/// DID を確定させ genesis op を準備する（ネットワーク通信なし）。
/// plc.directory への送信は `submit_plc_genesis` で別途行う。
///
/// `server_shared_rotation_key`はサーバー全体共有のrecovery鍵（`secrets.toml`由来）。
/// アカウント単位のローテーションキーはこの関数が新規生成し、`rotationKeys[0]`（主）に
/// 置く。共有鍵は`rotationKeys[1]`（副＝recovery用）としてのみ残る。
///
/// RFC 6979 による決定論的 ECDSA は user_signing_key/account_rotation_key が変わると
/// 署名も変わる。
pub fn prepare_plc_genesis(
    username: &str,
    pds_domain: &str,
    server_shared_rotation_key: &SigningKey,
) -> Result<PlcGenesis, PlcError> {
    let account_rotation_key = SigningKey::random(&mut OsRng);
    let account_rotation_did_key = p256_to_did_key(account_rotation_key.verifying_key());
    let server_shared_rotation_did_key =
        p256_to_did_key(server_shared_rotation_key.verifying_key());
    let rotation_keys = vec![account_rotation_did_key, server_shared_rotation_did_key];

    // ATPハンドルは常に小文字（`crate::username::to_atp_username` 参照。DNS/HTTPホスト名は
    // 経路上で小文字化されうるため、大文字混じりで PLC に登録すると恒久的に解決不能になる）。
    let handle = format!(
        "at://{}.{}",
        crate::username::to_atp_username(username),
        pds_domain
    );
    let pds_endpoint = format!("https://{}", pds_domain);

    let user_signing_key = SigningKey::random(&mut OsRng);
    let user_did_key = p256_to_did_key(user_signing_key.verifying_key());

    let mut verification_methods = BTreeMap::new();
    verification_methods.insert("atproto".to_string(), user_did_key);

    let mut services_unsigned = BTreeMap::new();
    services_unsigned.insert(
        "atproto_pds".to_string(),
        PlcService {
            endpoint: pds_endpoint.clone(),
            r#type: "AtprotoPersonalDataServer".to_string(),
        },
    );

    // ① 署名前オペレーションを DAG-CBOR エンコード → アカウント単位ローテーションキー
    //   （主鍵）で署名。genesis opは自己参照的にrotation_keysを検証するため、
    //   このオペレーションに列挙するどちらの鍵で署名しても有効だが、主鍵に統一する。
    let unsigned_op = GenesisOpUnsigned {
        also_known_as: vec![handle.clone()],
        prev: None,
        rotation_keys: rotation_keys.clone(),
        services: services_unsigned,
        r#type: "plc_operation".to_string(),
        verification_methods: verification_methods.clone(),
    };
    let unsigned_cbor =
        serde_ipld_dagcbor::to_vec(&unsigned_op).map_err(|e| PlcError::Cbor(e.to_string()))?;

    // low-S 正規化（AT Protocol/PLC の検証は low-S 必須。crates/seiran-common/src/atp/repo.rs の
    // create_commit と同じ理由）。
    let raw_sig: p256::ecdsa::Signature = account_rotation_key.sign(&unsigned_cbor);
    let raw_sig = raw_sig.normalize_s().unwrap_or(raw_sig);
    let sig_str = URL_SAFE_NO_PAD.encode(raw_sig.to_bytes().as_slice());

    // ② 署名済みオペレーションを DAG-CBOR エンコード → SHA-256 → DID
    let mut services_signed = BTreeMap::new();
    services_signed.insert(
        "atproto_pds".to_string(),
        PlcService {
            endpoint: pds_endpoint.clone(),
            r#type: "AtprotoPersonalDataServer".to_string(),
        },
    );
    let signed_op = GenesisOpSigned {
        also_known_as: vec![handle.clone()],
        prev: None,
        rotation_keys,
        services: services_signed,
        sig: sig_str,
        r#type: "plc_operation".to_string(),
        verification_methods,
    };
    let signed_cbor =
        serde_ipld_dagcbor::to_vec(&signed_op).map_err(|e| PlcError::Cbor(e.to_string()))?;
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
    let rotation_key_pem = account_rotation_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| PlcError::KeyGen(e.to_string()))?
        .to_string();

    Ok(PlcGenesis {
        did,
        signing_key_pem,
        rotation_key_pem,
        signed_op,
    })
}

/// 準備済み genesis op を plc.directory に送信する（ネットワーク通信あり）。
pub async fn submit_plc_genesis(
    genesis: &PlcGenesis,
    client: &reqwest::Client,
) -> Result<(), PlcError> {
    let url = format!("{}/{}", plc_directory_base_url(), genesis.did);
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

/// 現在のDIDドキュメント全体（`/{did}/data`）と直前オペレーションのCID（`/{did}/log/audit`
/// の最終エントリの`cid`フィールド）を取得する。転出元API対応（アカウント単位ローテー
/// ションキーへのバックフィル、`signPlcOperation`実装）の両方で使う更新オペレーション
/// 生成の前段。
pub async fn fetch_current_plc_doc_and_prev(
    did: &str,
    client: &reqwest::Client,
) -> Result<(serde_json::Value, String), PlcError> {
    let base = plc_directory_base_url();

    let data_url = format!("{base}/{did}/data");
    let current_data: serde_json::Value = client
        .get(&data_url)
        .send()
        .await
        .map_err(|e| PlcError::Http(format!("/data取得失敗: {e}")))?
        .json()
        .await
        .map_err(|e| PlcError::Http(format!("/dataパース失敗: {e}")))?;

    let audit_url = format!("{base}/{did}/log/audit");
    let audit_log: Vec<serde_json::Value> = client
        .get(&audit_url)
        .send()
        .await
        .map_err(|e| PlcError::Http(format!("/log/audit取得失敗: {e}")))?
        .json()
        .await
        .map_err(|e| PlcError::Http(format!("/log/auditパース失敗: {e}")))?;
    let prev = audit_log
        .last()
        .and_then(|op| op.get("cid"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| PlcError::Http("/log/auditにcidが見つからない".to_string()))?
        .to_string();

    Ok((current_data, prev))
}

/// アカウント単位ローテーションキーへのバックフィル（転出元API対応の前提、Phase A）と
/// `com.atproto.identity.signPlcOperation`（Phase B）の両方が使う更新オペレーション生成。
/// `also_known_as`/`verification_methods`/`services`は`None`なら既存のDIDドキュメント
/// （`current_data`、`fetch_current_plc_doc_and_prev`で取得）の値にフォールバックする
/// （`signPlcOperation`の入力で省略されたフィールドの扱い。バックフィルは常に全て`None`
/// で呼び、既存値を丸ごと維持する）。`rotationKeys`は呼び出し元が確定済みの最終形を渡す
/// （フォールバックなし、常に明示指定）。署名鍵は呼び出し元が渡す（バックフィルは現在
/// 有効な共有鍵、`signPlcOperation`はアカウント単位鍵）。
#[allow(clippy::too_many_arguments)]
pub fn prepare_plc_rotation_update(
    current_data: &serde_json::Value,
    prev: &str,
    new_rotation_keys: Vec<String>,
    also_known_as_override: Option<Vec<String>>,
    verification_methods_override: Option<serde_json::Value>,
    services_override: Option<serde_json::Value>,
    signing_key: &SigningKey,
) -> Result<serde_json::Value, PlcError> {
    let also_known_as: Vec<String> = match also_known_as_override {
        Some(v) => v,
        None => current_data
            .get("alsoKnownAs")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let verification_methods: BTreeMap<String, String> = match verification_methods_override {
        Some(v) => serde_json::from_value(v)
            .map_err(|e| PlcError::Cbor(format!("verificationMethodsパース失敗: {e}")))?,
        None => current_data
            .get("verificationMethods")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| PlcError::Cbor(format!("verificationMethodsパース失敗: {e}")))?
            .unwrap_or_default(),
    };
    let services: BTreeMap<String, PlcService> = match services_override {
        Some(v) => serde_json::from_value(v)
            .map_err(|e| PlcError::Cbor(format!("servicesパース失敗: {e}")))?,
        None => current_data
            .get("services")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| PlcError::Cbor(format!("servicesパース失敗: {e}")))?
            .unwrap_or_default(),
    };

    let unsigned_op = UpdateOpUnsigned {
        also_known_as: also_known_as.clone(),
        prev: prev.to_string(),
        rotation_keys: new_rotation_keys.clone(),
        services: services.clone(),
        r#type: "plc_operation".to_string(),
        verification_methods: verification_methods.clone(),
    };
    let unsigned_cbor =
        serde_ipld_dagcbor::to_vec(&unsigned_op).map_err(|e| PlcError::Cbor(e.to_string()))?;

    let raw_sig: p256::ecdsa::Signature = signing_key.sign(&unsigned_cbor);
    let raw_sig = raw_sig.normalize_s().unwrap_or(raw_sig);
    let sig_str = URL_SAFE_NO_PAD.encode(raw_sig.to_bytes().as_slice());

    let signed_op = UpdateOpSigned {
        also_known_as,
        prev: prev.to_string(),
        rotation_keys: new_rotation_keys,
        services,
        sig: sig_str,
        r#type: "plc_operation".to_string(),
        verification_methods,
    };
    let signed_json = serde_json::to_value(&signed_op)
        .map_err(|e| PlcError::Cbor(format!("JSON変換失敗: {e}")))?;

    Ok(signed_json)
}

/// 既に署名済みの任意のPLCオペレーション（`serde_json::Value`）をplc.directoryへ提出する。
/// `submit_plc_genesis`と同じエンドポイント（`{plc_directory_base_url()}/{did}`）を使う、
/// より汎用な版。転入フローの`com.atproto.identity.submitPlcOperation`で、PDS Aの
/// `signPlcOperation`が返した署名済みオペレーションをそのまま提出するために使う。
pub async fn submit_plc_operation_raw(
    did: &str,
    operation: &serde_json::Value,
    client: &reqwest::Client,
) -> Result<(), PlcError> {
    let url = format!("{}/{}", plc_directory_base_url(), did);
    let res = client
        .post(&url)
        .json(operation)
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
