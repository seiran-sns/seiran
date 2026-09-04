//! ActivityPub クライアント ＆ HTTP Signatures 署名検証モジュール
//!
//! リモートアクタードキュメントの取得、公開鍵（RSA）のフェッチとキャッシュ、
//! および受信リクエストの HTTP Signatures 署名検証を行う。

use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::{Pkcs1v15Sign, RsaPrivateKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 公開鍵キャッシュの有効期限。リモートサーバーが鍵をローテーションしても、
/// 最大でもこの時間が経てば新しい鍵を再フェッチするようになる
/// （`verify_signature` は加えて検証失敗時に1回だけ強制再フェッチも行う）。
const KEY_CACHE_TTL: Duration = Duration::from_secs(3600);

/// AP 通信エラー
#[derive(Debug, thiserror::Error)]
pub enum ApError {
    #[error("HTTP エラー: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON パースエラー: {0}")]
    Json(#[from] serde_json::Error),
    #[error("署名エラー: {0}")]
    Signature(String),
    #[error("アクター取得失敗: {0}")]
    FetchActor(String),
    #[error("オブジェクトが削除済み: {0}")]
    Gone(String),
    #[error("{0}")]
    Other(String),
}

/// 後方互換性のため `Result<_, String>` コンテキストで `?` が使えるようにする
impl From<ApError> for String {
    fn from(e: ApError) -> Self {
        e.to_string()
    }
}

/// ジョブのリトライ判断（`crate::traits::JobError`）向けの分類。
/// - `Http`: 接続断・タイムアウト等はリトライで回復し得る（Transient）。
/// - `Json`: リモートが返す本文が壊れている場合、リトライしても直らない（Permanent）。
/// - `Signature`: 自インスタンスの署名鍵設定が原因のため、リトライしても直らない（Permanent）。
/// - `FetchActor` / `Other`: リモートアクター解決失敗は一時的な到達不能・恒久的な404の
///   両方があり得て呼び出し側では判別できないため、安全側（見逃しよりリトライ超過の方が無害）
///   に倒して Transient のままにする。
/// - `Gone`: 404/410が明示的に確認できているため、リトライしても直らない（Permanent）。
impl From<ApError> for crate::traits::JobError {
    fn from(e: ApError) -> Self {
        match &e {
            ApError::Json(_) | ApError::Signature(_) | ApError::Gone(_) => {
                crate::traits::JobError::Permanent(e.to_string())
            }
            ApError::Http(_) | ApError::FetchActor(_) | ApError::Other(_) => {
                crate::traits::JobError::Transient(e.to_string())
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PublicKeyInfo {
    pub id: String,
    pub owner: String,
    #[serde(rename = "publicKeyPem")]
    pub public_key_pem: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApActor {
    pub id: String,
    #[serde(rename = "type")]
    pub actor_type: String,
    #[serde(rename = "preferredUsername")]
    pub preferred_username: Option<String>,
    pub name: Option<String>,
    pub summary: Option<String>,
    /// アバター画像。実装により object / array / 欠落があり得るため Value で受ける
    /// （`avatar_url()` で URL を抽出する）。
    #[serde(default)]
    pub icon: Option<serde_json::Value>,
    pub inbox: Option<String>,
    pub outbox: Option<String>,
    /// ピン留め投稿の OrderedCollection。実装により URL 文字列（Mastodon 等）と
    /// OrderedCollection オブジェクトのインライン埋め込み（bridgy-fed 等）の両方が
    /// あり得るため `Value` で受ける（`fetch_ap_featured` で吸収する）。無い実装
    /// （Mastodon 以前や一部の軽量実装）もあるため `Option`。
    #[serde(default)]
    pub featured: Option<serde_json::Value>,
    /// フォロー中一覧の OrderedCollection URL（#68）。非公開設定にしている実装や
    /// 未対応実装もあるため `Option`。
    #[serde(default)]
    pub following: Option<String>,
    /// フォロワー一覧の OrderedCollection URL（#68）。`following` と同様の理由で `Option`。
    #[serde(default)]
    pub followers: Option<String>,
    #[serde(rename = "publicKey")]
    pub public_key: Option<PublicKeyInfo>,
    /// 表示名(`name`)・自己紹介(`summary`)中のカスタム絵文字タグ(`type:"Emoji"`)。
    /// `emoji_map()` で `{shortcode: 画像URL}` に変換する。
    #[serde(default)]
    pub tag: Vec<serde_json::Value>,
    /// プロフィールのキーバリュー項目（#62）。`type: "PropertyValue"` の要素を
    /// `property_values()` で `(name, value)` のペアに変換する。
    #[serde(default)]
    pub attachment: Vec<serde_json::Value>,
    /// アカウント引っ越し（Move）の検証に使う「この身元が別名として名乗っているURI」一覧。
    /// 実装によって単一文字列/配列のどちらでも来うるため `deserialize_string_or_vec` で吸収する。
    #[serde(
        rename = "alsoKnownAs",
        default,
        deserialize_with = "deserialize_string_or_vec"
    )]
    pub also_known_as: Vec<String>,
    /// 鍵アカウント（フォロー承認制）かどうか。フィールド自体が省略された場合は
    /// `false`（非鍵アカウント）として扱う（AS2の一般的な省略時解釈）。
    #[serde(rename = "manuallyApprovesFollowers", default)]
    pub manually_approves_followers: bool,
    /// リモートseiranアクターの相互申告マージ用（seiran独自拡張、#236）。このアクター自身が
    /// 「自分のAT ProtocolでのDIDはこれだ」と自己申告する値。一方的な自己申告に過ぎず、
    /// ATP側の`org.seiran.actor.declaration`宣言レコードがこのAP Actor URIを指し返して
    /// いる場合にのみ相互一致とみなす（`docs/protocols.md` 11節参照）。
    #[serde(rename = "seiranAtDid", default)]
    pub seiran_at_did: Option<String>,
}

/// AS2の`alsoKnownAs`等、実装により単一文字列/配列のどちらでも来うるフィールド用の
/// 寛容なデシリアライザ。
fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        Single(String),
        Multiple(Vec<String>),
    }
    Ok(match Option::<StringOrVec>::deserialize(deserializer)? {
        Some(StringOrVec::Single(s)) => vec![s],
        Some(StringOrVec::Multiple(v)) => v,
        None => Vec::new(),
    })
}

impl ApActor {
    /// `icon`（object または array）から最初の画像 URL を抽出する。
    pub fn avatar_url(&self) -> Option<String> {
        let v = self.icon.as_ref()?;
        let obj = if v.is_array() {
            v.as_array()?.first()?
        } else {
            v
        };
        obj.get("url")?.as_str().map(|s| s.to_string())
    }

    /// 表示名中のカスタム絵文字の shortcode→画像URLマップ。
    pub fn emoji_map(&self) -> serde_json::Value {
        build_emoji_map(&self.tag)
    }

    /// `attachment` 配列（`type: "PropertyValue"` の要素）から `(name, value)` のペアを
    /// 抽出する（#62）。`value` は HTML を含みうるため（Mastodon 等はリンクを `<a>` タグ付きで
    /// 送る）、呼び出し側で必要に応じてプレーンテキスト化すること。
    pub fn property_values(&self) -> Vec<(String, String)> {
        self.attachment
            .iter()
            .filter(|a| a.get("type").and_then(|t| t.as_str()) == Some("PropertyValue"))
            .filter_map(|a| {
                let name = a.get("name")?.as_str()?.to_string();
                let value = a.get("value")?.as_str()?.to_string();
                Some((name, value))
            })
            .collect()
    }

    /// `alsoKnownAs` に指定のURIが含まれるか（Move受信時の本人確認、`docs/protocols.md`参照）。
    pub fn claims_also_known_as(&self, uri: &str) -> bool {
        self.also_known_as.iter().any(|a| a == uri)
    }

    /// `property_values()` を `MAX_PROFILE_FIELDS` 件までに切り詰め、`value` を `strip_html`
    /// でプレーンテキスト化した上で `actors.profile_fields` へそのまま保存できる JSON 配列
    /// （`[{"name": ..., "value": ...}, ...]`）を組み立てる（#62）。
    pub fn profile_fields_json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.property_values()
                .into_iter()
                .filter_map(|(name, value)| {
                    // strip_html 後に空になる値（アイコンのみのリンク等）は取り込まない。
                    let value = crate::jobs::inbound_activity_process::strip_html(&value);
                    if value.trim().is_empty() {
                        None
                    } else {
                        Some(serde_json::json!({"name": name, "value": value}))
                    }
                })
                .take(crate::MAX_PROFILE_FIELDS)
                .collect(),
        )
    }
}

/// AP の `tag` 配列（`type:"Emoji"` の要素）から `{shortcode: 画像URL}` のマップを構築する。
/// Note 本文・Person 表示名・Like/EmojiReact のいずれでも同じ形式で使われる:
/// `{"id":"...", "type":"Emoji", "name":":shortcode:", "icon":{"type":"Image","url":"..."}}`
pub fn build_emoji_map(tags: &[serde_json::Value]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for tag in tags {
        if tag["type"].as_str() != Some("Emoji") {
            continue;
        }
        if let (Some(name), Some(url)) = (tag["name"].as_str(), tag["icon"]["url"].as_str()) {
            map.insert(name.to_string(), serde_json::Value::String(url.to_string()));
        }
    }
    serde_json::Value::Object(map)
}

/// AP Note の `to`/`cc` から Mastodon 互換の4値可視性を判定する。
/// - `to` に Public が含まれる: `public`（一般的な公開投稿）
/// - `cc` にのみ Public が含まれる: `unlisted`（公開だが一覧に載らない）
/// - Public がどちらにも無く `to` にフォロワーコレクション（`.../followers`）が含まれる: `followers_only`
/// - それ以外（特定アクターのみ宛先）: `direct`
pub fn classify_ap_visibility(to: &[String], cc: &[String]) -> &'static str {
    const PUBLIC_URIS: [&str; 3] = [
        "https://www.w3.org/ns/activitystreams#Public",
        "as:Public",
        "Public",
    ];
    let has_public = |uris: &[String]| uris.iter().any(|u| PUBLIC_URIS.contains(&u.as_str()));

    if has_public(to) {
        "public"
    } else if has_public(cc) {
        "unlisted"
    } else if to.iter().any(|u| u.ends_with("/followers")) {
        "followers_only"
    } else {
        "direct"
    }
}

/// ActivityPub 通信クライアント
///
/// HTTP クライアントと公開鍵キャッシュをインスタンスフィールドとして保持する。
/// プロセスグローバルな静的キャッシュを廃止し、テスト時にモックを注入できる構造にした。
pub struct ApClient {
    pub http: Arc<reqwest::Client>,
    pub key_cache: Arc<RwLock<HashMap<String, (String, Instant)>>>,
}

impl ApClient {
    pub fn new(http: Arc<reqwest::Client>) -> Self {
        Self {
            http,
            key_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// リモートアクター情報を取得する
    pub async fn fetch_actor(&self, actor_uri: &str) -> Result<ApActor, ApError> {
        let res = self
            .http
            .get(actor_uri)
            .header("Accept", "application/activity+json, application/ld+json")
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(ApError::FetchActor(format!("ステータス {}", res.status())));
        }

        let actor = res.json::<ApActor>().await?;
        Ok(actor)
    }

    /// リモートアクター情報を HTTP Signatures 付き GET で取得する。`fetch_actor`と同じだが、
    /// Authorized Fetch（secure mode）を要求するインスタンス（songbird.cloud等）は
    /// 未署名GETに401を返すため、`upsert_remote_fedi_actor`（Follow/Create/Like/EmojiReact/
    /// Announce等すべての受信経路が投稿・リアクション送信元アクターの解決に使う共通処理）
    /// はこちらを使う。
    pub async fn fetch_actor_signed(
        &self,
        actor_uri: &str,
        signing_key: (&str, &str),
    ) -> Result<ApActor, ApError> {
        let res = self
            .signed_get(actor_uri, signing_key.0, signing_key.1)
            .await?;

        if !res.status().is_success() {
            return Err(ApError::FetchActor(format!("ステータス {}", res.status())));
        }

        let actor = res.json::<ApActor>().await?;
        Ok(actor)
    }

    /// 指定した key_id (URL) から公開鍵 PEM を取得する（TTL付きキャッシュ対応）。
    /// `signing_key` があれば HTTP Signatures 付き GET で取得する（Authorized Fetch/secure mode
    /// を要求する送信元、例: songbird.cloud からの受信でも検証できるように）。
    pub async fn get_public_key_pem(
        &self,
        key_id: &str,
        signing_key: Option<(&str, &str)>,
    ) -> Result<String, ApError> {
        // 1. キャッシュヒット確認（TTL内のみ有効）
        {
            let cache = self.key_cache.read().await;
            if let Some((pem, fetched_at)) = cache.get(key_id) {
                if fetched_at.elapsed() < KEY_CACHE_TTL {
                    return Ok(pem.clone());
                }
            }
        }

        self.fetch_and_cache_public_key_pem(key_id, signing_key)
            .await
    }

    /// キャッシュの有無・TTLを無視して公開鍵を再フェッチし、結果でキャッシュを上書きする。
    /// リモートの鍵ローテーション後に署名検証が失敗した際のリトライで使う。
    async fn fetch_and_cache_public_key_pem(
        &self,
        key_id: &str,
        signing_key: Option<(&str, &str)>,
    ) -> Result<String, ApError> {
        tracing::info!("[ApClient] 公開鍵フェッチ中: {}", key_id);

        // アクターもしくは鍵を直接フェッチする。
        // 通常 key_id (e.g. https://example.com/users/test#main-key) にアクセスすると
        // アクター情報そのもの、あるいは鍵オブジェクト単体が返る。
        // フラグメント部分 (#main-key) を除外したベースURIを叩くのが安全。
        let base_uri = key_id.split('#').next().unwrap_or(key_id);
        let actor = match signing_key {
            Some(key) => self.fetch_actor_signed(base_uri, key).await?,
            None => self.fetch_actor(base_uri).await?,
        };

        if let Some(pubkey_info) = actor.public_key {
            if pubkey_info.id == key_id || base_uri == pubkey_info.owner {
                let pem = pubkey_info.public_key_pem;
                let mut cache = self.key_cache.write().await;
                cache.insert(key_id.to_string(), (pem.clone(), Instant::now()));
                return Ok(pem);
            }
        }

        Err(ApError::FetchActor(format!(
            "取得したアクタードキュメントから一致する key_id ({}) が見つかりません",
            key_id
        )))
    }

    /// HTTP Signatures の署名を検証します
    ///
    /// # 引数
    /// - `method`: リクエストメソッド (e.g. "POST")
    /// - `path`: リクエストパス (e.g. "/inbox")
    /// - `headers`: 受信した HTTP ヘッダー一覧
    /// - `signature_header`: 受信した `Signature` ヘッダーの内容
    /// - `signing_key`: 送信元の公開鍵取得（`keyId`へのGET）に使う署名鍵。Authorized Fetch
    ///   （secure mode）を要求するインスタンスからの受信でも検証できるようにする。
    pub async fn verify_signature(
        &self,
        method: &str,
        path: &str,
        headers: &HashMap<String, String>,
        signature_header: &str,
        signing_key: Option<(&str, &str)>,
    ) -> Result<bool, ApError> {
        // 1. Signature ヘッダーの要素をパース
        // 例: keyId="...",algorithm="rsa-sha256",headers="...",signature="..."
        let parsed = parse_signature_header(signature_header)?;
        let key_id = parsed
            .get("keyId")
            .ok_or_else(|| ApError::Signature("keyId が見つかりません".to_string()))?;
        let signature_b64 = parsed
            .get("signature")
            .ok_or_else(|| ApError::Signature("signature が見つかりません".to_string()))?;
        let header_list_str = parsed
            .get("headers")
            .cloned()
            .unwrap_or_else(|| "date".to_string());

        // 2. 署名対象文字列 (Signing String) を構築
        let signing_string = build_signing_string(method, path, headers, &header_list_str)?;

        // 3. 署名の base64 デコード（鍵の取得元によらず共通）
        let signature_bytes =
            base64::Engine::decode(&base64::prelude::BASE64_STANDARD, signature_b64)
                .map_err(|e| ApError::Signature(format!("署名base64デコード失敗: {}", e)))?;

        // 4. 公開鍵 PEM の取得（キャッシュ利用）と検証
        let pem = self.get_public_key_pem(key_id, signing_key).await?;
        if Self::verify_with_pem(&pem, &signing_string, &signature_bytes).is_ok() {
            return Ok(true);
        }

        // 5. キャッシュ済みの鍵での検証に失敗した場合、リモートが鍵をローテーションした
        // 可能性があるため、キャッシュを無視して1回だけ再フェッチし再検証する。
        // 同じ鍵しか得られなかった場合は無駄な再検証をせず最初の失敗をそのまま返す。
        let fresh_pem = self
            .fetch_and_cache_public_key_pem(key_id, signing_key)
            .await?;
        if fresh_pem == pem {
            return Err(ApError::Signature("署名検証失敗".to_string()));
        }
        Self::verify_with_pem(&fresh_pem, &signing_string, &signature_bytes).map(|()| true)
    }

    /// 与えられた公開鍵 PEM で signing string の署名を検証する（純粋な検証処理部分）
    ///
    /// Pleroma はアクタードキュメントの `publicKeyPem` の末尾に余分な空行を付けて返す
    /// ことがあり、`rsa` クレートの PEM パーサはこれを `PreEncapsulationBoundary`
    /// エラーとして拒否する（実例: post.syobon.net、2026-09-03確認）。`trim()` して
    /// から渡すことで、Mastodon/Misskey 等の余分な空白がないPEMと同様に扱う。
    fn verify_with_pem(
        pem: &str,
        signing_string: &str,
        signature_bytes: &[u8],
    ) -> Result<(), ApError> {
        let public_key = RsaPublicKey::from_public_key_pem(pem.trim())
            .map_err(|e| ApError::Signature(format!("RSA公開鍵のパース失敗: {}", e)))?;

        let mut hasher = Sha256::new();
        hasher.update(signing_string.as_bytes());
        let hashed = hasher.finalize();

        public_key
            .verify(Pkcs1v15Sign::new::<Sha256>(), &hashed, signature_bytes)
            .map_err(|e| ApError::Signature(format!("署名検証失敗: {:?}", e)))
    }

    /// HTTP Signatures 付きで GET する。POST 用の署名（`(request-target) host date
    /// content-type digest`）と異なり、GET にはボディが無いため `digest`/`content-type`
    /// を含めない `(request-target) host date` の3つのみを署名対象にする
    /// （Misskeyの`createSignedGet`と同じ組み合わせ）。
    pub(crate) async fn signed_get(
        &self,
        url: &str,
        actor_key_id: &str,
        private_key_pem: &str,
    ) -> Result<reqwest::Response, ApError> {
        let parsed_url =
            url::Url::parse(url).map_err(|e| ApError::Other(format!("URL パースエラー: {}", e)))?;
        let host = parsed_url.host_str().unwrap_or("").to_string();
        let path = parsed_url.path().to_string();

        let now = chrono::Utc::now();
        let date_str = now.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

        let signing_string = format!(
            "(request-target): get {}\nhost: {}\ndate: {}",
            path, host, date_str
        );

        // RSA鍵パース・署名計算はCPUバウンドの同期処理（実測 約50ms/回）。コレクション
        // ページネーション（`fetch_ap_collection_uris`等）はページごとにこれを呼ぶため、
        // async関数内で直接実行するとtokioワーカースレッドを長時間ブロックし、同時実行中の
        // 他リクエスト（`tokio::time::timeout`のタイマー含む）まで巻き込んで遅延させる
        // （2026-08-31実測、プロフィール表示が数秒〜10秒規模に劣化した不具合の原因）。
        // spawn_blockingで専用スレッドプールへ逃がす。
        let actor_key_id_owned = actor_key_id.to_string();
        let private_key_pem_owned = private_key_pem.to_string();
        let signature_header = tokio::task::spawn_blocking(move || -> Result<String, ApError> {
            let private_key = RsaPrivateKey::from_pkcs8_pem(&private_key_pem_owned)
                .map_err(|e| ApError::Signature(format!("RSA 秘密鍵パース失敗: {}", e)))?;

            let mut hasher = Sha256::new();
            hasher.update(signing_string.as_bytes());
            let hashed = hasher.finalize();

            let sig_bytes = private_key
                .sign(Pkcs1v15Sign::new::<Sha256>(), &hashed)
                .map_err(|e| ApError::Signature(format!("RSA 署名失敗: {}", e)))?;

            let sig_b64 = base64::Engine::encode(&base64::prelude::BASE64_STANDARD, sig_bytes);

            Ok(format!(
                r#"keyId="{}",algorithm="rsa-sha256",headers="(request-target) host date",signature="{}""#,
                actor_key_id_owned, sig_b64
            ))
        })
        .await
        .map_err(|e| ApError::Signature(format!("spawn_blocking 失敗: {}", e)))??;

        let res = self
            .http
            .get(url)
            .header("Accept", "application/activity+json, application/ld+json")
            .header("Date", &date_str)
            .header("Host", &host)
            .header("Signature", &signature_header)
            .send()
            .await?;

        Ok(res)
    }

    /// HTTP Signatures 付きで ActivityPub エンドポイントへ POST する
    ///
    /// # 引数
    /// - `url`: 送信先 URL（相手の inbox 等）
    /// - `body`: JSON 文字列
    /// - `actor_key_id`: 署名に使うキー ID（例: `https://beta.seiran.org/users/yubaj#main-key`）
    /// - `private_key_pem`: RSA 秘密鍵 PEM
    pub async fn sign_and_post(
        &self,
        url: &str,
        body: &str,
        actor_key_id: &str,
        private_key_pem: &str,
    ) -> Result<(), ApError> {
        let now = chrono::Utc::now();
        let date_str = now.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

        let parsed_url =
            url::Url::parse(url).map_err(|e| ApError::Other(format!("URL パースエラー: {}", e)))?;
        let host = parsed_url.host_str().unwrap_or("").to_string();
        let path = parsed_url.path().to_string();

        // Digest ヘッダー（SHA-256 of body）
        let body_hash = Sha256::digest(body.as_bytes());
        let digest = format!(
            "SHA-256={}",
            base64::Engine::encode(&base64::prelude::BASE64_STANDARD, body_hash)
        );

        // 署名対象文字列
        let signing_string = format!(
            "(request-target): post {}\nhost: {}\ndate: {}\ncontent-type: application/activity+json\ndigest: {}",
            path, host, date_str, digest
        );

        // RSA-SHA256 署名
        let private_key = RsaPrivateKey::from_pkcs8_pem(private_key_pem)
            .map_err(|e| ApError::Signature(format!("RSA 秘密鍵パース失敗: {}", e)))?;

        let mut hasher = Sha256::new();
        hasher.update(signing_string.as_bytes());
        let hashed = hasher.finalize();

        let sig_bytes = private_key
            .sign(Pkcs1v15Sign::new::<Sha256>(), &hashed)
            .map_err(|e| ApError::Signature(format!("RSA 署名失敗: {}", e)))?;

        let sig_b64 = base64::Engine::encode(&base64::prelude::BASE64_STANDARD, sig_bytes);

        let signature_header = format!(
            r#"keyId="{}",algorithm="rsa-sha256",headers="(request-target) host date content-type digest",signature="{}""#,
            actor_key_id, sig_b64
        );

        let res = self
            .http
            .post(url)
            .header("Date", &date_str)
            .header("Host", &host)
            .header("Content-Type", "application/activity+json")
            .header("Digest", &digest)
            .header("Signature", &signature_header)
            .body(body.to_string())
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let body_text = res.text().await.unwrap_or_default();
            return Err(ApError::Other(format!(
                "POST レスポンスエラー {}: {}",
                status, body_text
            )));
        }

        Ok(())
    }

    /// リモート AP オブジェクト（Note 等）を URI から取得する。
    /// `signing_key`（キーID, RSA秘密鍵PEM）で HTTP Signatures 付き GET
    /// （Authorized Fetch/secure mode対応、Misskeyの`ApRequestCreator#createSignedGet`と同形）
    /// を送る。songbird.cloud 等 `AUTHORIZED_FETCH` を有効にしたインスタンスは未署名GETに
    /// 401 `Request not signed` を返し、参照解決（#233等）が pending のまま固着するため。
    pub async fn fetch_object(
        &self,
        object_uri: &str,
        signing_key: (&str, &str),
    ) -> Result<serde_json::Value, ApError> {
        let res = self
            .signed_get(object_uri, signing_key.0, signing_key.1)
            .await?;

        if matches!(
            res.status(),
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE
        ) {
            return Err(ApError::Gone(format!(
                "ステータス {} ({})",
                res.status(),
                object_uri
            )));
        }
        if !res.status().is_success() {
            return Err(ApError::Other(format!(
                "オブジェクト取得失敗: ステータス {} ({})",
                res.status(),
                object_uri
            )));
        }

        let obj = res.json::<serde_json::Value>().await?;
        Ok(obj)
    }

    /// 署名鍵があればHTTP Signatures付き、無ければ従来通り未署名のGETを送る汎用ヘルパー。
    /// `fetch_ap_collection_uris`（followers/following）・`fetch_ap_history`（outbox過去ログ）・
    /// `fetch_ap_featured`（ピン留め）等、ページネーションを自前で辿るフェッチ経路で使う。
    pub async fn get_maybe_signed(
        &self,
        url: &str,
        signing_key: Option<(&str, &str)>,
    ) -> Result<reqwest::Response, ApError> {
        match signing_key {
            Some((key_id, pem)) => self.signed_get(url, key_id, pem).await,
            None => Ok(self
                .http
                .get(url)
                .header("Accept", "application/activity+json, application/ld+json")
                .send()
                .await?),
        }
    }

    /// Webfinger 解決を実行する
    pub async fn resolve_webfinger(&self, username: &str, domain: &str) -> Result<String, ApError> {
        super::webfinger::resolve_webfinger_impl(&self.http, username, domain).await
    }
}

/// Signature ヘッダーを簡易パースする
fn parse_signature_header(header: &str) -> Result<HashMap<String, String>, ApError> {
    let mut map = HashMap::new();
    // カンマ区切りの key="value" パターンを取り出す
    // 簡易的にクォーテーションを考慮しつつ分割する
    let parts = header.split(',');
    for part in parts {
        let kv: Vec<&str> = part.splitn(2, '=').collect();
        if kv.len() == 2 {
            let key = kv[0].trim().to_string();
            let val = kv[1].trim().trim_matches('"').to_string();
            map.insert(key, val);
        }
    }
    Ok(map)
}

/// 署名ヘッダーに列挙された順に署名対象文字列を構築する
fn build_signing_string(
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    header_list_str: &str,
) -> Result<String, ApError> {
    let mut lines = Vec::new();
    for header_name in header_list_str.split(' ') {
        let name_lower = header_name.to_lowercase();
        if name_lower == "(request-target)" {
            lines.push(format!(
                "(request-target): {} {}",
                method.to_lowercase(),
                path
            ));
        } else {
            let val = headers.get(&name_lower).ok_or_else(|| {
                ApError::Signature(format!(
                    "署名対象ヘッダー \"{}\" がリクエストに見つかりません",
                    header_name
                ))
            })?;
            lines.push(format!("{}: {}", name_lower, val));
        }
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::JobError;

    // ─── ApError → JobError（リトライ判断）─────────────────────────────────

    #[test]
    fn json_and_signature_errors_are_permanent() {
        let json_err: serde_json::Error = serde_json::from_str::<()>("not json").unwrap_err();
        assert!(JobError::from(ApError::Json(json_err)).is_permanent());
        assert!(JobError::from(ApError::Signature("bad key".to_string())).is_permanent());
    }

    #[test]
    fn fetch_actor_and_other_errors_are_transient() {
        assert!(!JobError::from(ApError::FetchActor("timeout".to_string())).is_permanent());
        assert!(!JobError::from(ApError::Other("unknown".to_string())).is_permanent());
    }

    // ─── parse_signature_header ───────────────────────────────────────────

    #[test]
    fn parse_signature_header_extracts_key_fields() {
        let header = r#"keyId="https://example.com/users/alice#main-key",algorithm="rsa-sha256",headers="(request-target) host date",signature="abc123""#;
        let map = parse_signature_header(header).unwrap();
        assert_eq!(
            map.get("keyId").map(|s| s.as_str()),
            Some("https://example.com/users/alice#main-key")
        );
        assert_eq!(map.get("algorithm").map(|s| s.as_str()), Some("rsa-sha256"));
        assert_eq!(
            map.get("headers").map(|s| s.as_str()),
            Some("(request-target) host date")
        );
        assert_eq!(map.get("signature").map(|s| s.as_str()), Some("abc123"));
    }

    #[test]
    fn parse_signature_header_single_pair() {
        let header = r#"keyId="did:example:123#key-1""#;
        let map = parse_signature_header(header).unwrap();
        assert_eq!(
            map.get("keyId").map(|s| s.as_str()),
            Some("did:example:123#key-1")
        );
    }

    #[test]
    fn parse_signature_header_returns_empty_on_malformed() {
        let map = parse_signature_header("no-equals-sign").unwrap();
        assert!(map.is_empty());
    }

    // ─── build_signing_string ─────────────────────────────────────────────

    #[test]
    fn build_signing_string_request_target() {
        let headers = HashMap::new();
        let result = build_signing_string("POST", "/inbox", &headers, "(request-target)").unwrap();
        assert_eq!(result, "(request-target): post /inbox");
    }

    #[test]
    fn build_signing_string_multiple_headers() {
        let mut headers = HashMap::new();
        headers.insert("host".to_string(), "example.com".to_string());
        headers.insert(
            "date".to_string(),
            "Mon, 01 Jan 2024 00:00:00 GMT".to_string(),
        );
        let result =
            build_signing_string("POST", "/inbox", &headers, "(request-target) host date").unwrap();
        let expected =
            "(request-target): post /inbox\nhost: example.com\ndate: Mon, 01 Jan 2024 00:00:00 GMT";
        assert_eq!(result, expected);
    }

    #[test]
    fn build_signing_string_method_is_lowercased() {
        let headers = HashMap::new();
        let result =
            build_signing_string("GET", "/users/alice", &headers, "(request-target)").unwrap();
        assert!(result.starts_with("(request-target): get "));
    }

    #[test]
    fn build_signing_string_missing_header_returns_error() {
        let headers = HashMap::new();
        let err = build_signing_string("POST", "/inbox", &headers, "host").unwrap_err();
        assert!(matches!(err, ApError::Signature(_)));
    }

    // ─── build_emoji_map ───────────────────────────────────────────

    #[test]
    fn build_emoji_map_extracts_multiple_shortcodes() {
        let tags = vec![
            serde_json::json!({
                "type": "Emoji", "name": ":blobcat:",
                "icon": { "url": "https://example.com/blobcat.png" }
            }),
            serde_json::json!({
                "type": "Emoji", "name": ":ablobcatwave:",
                "icon": { "url": "https://example.com/wave.png" }
            }),
        ];
        let map = build_emoji_map(&tags);
        assert_eq!(map[":blobcat:"], "https://example.com/blobcat.png");
        assert_eq!(map[":ablobcatwave:"], "https://example.com/wave.png");
    }

    #[test]
    fn build_emoji_map_ignores_non_emoji_tags() {
        let tags = vec![serde_json::json!({
            "type": "Mention", "name": "@alice", "href": "https://example.com/users/alice"
        })];
        assert_eq!(build_emoji_map(&tags), serde_json::json!({}));
    }

    #[test]
    fn build_emoji_map_empty_tags() {
        assert_eq!(build_emoji_map(&[]), serde_json::json!({}));
    }

    #[test]
    fn ap_actor_emoji_map_uses_tag_field() {
        let actor: ApActor = serde_json::from_value(serde_json::json!({
            "id": "https://example.com/users/alice",
            "type": "Person",
            "tag": [
                { "type": "Emoji", "name": ":blobcat:", "icon": { "url": "https://example.com/blobcat.png" } }
            ]
        })).unwrap();
        assert_eq!(
            actor.emoji_map()[":blobcat:"],
            "https://example.com/blobcat.png"
        );
    }

    #[test]
    fn ap_actor_manually_approves_followers_defaults_to_false_when_absent() {
        let actor: ApActor = serde_json::from_value(serde_json::json!({
            "id": "https://example.com/users/alice",
            "type": "Person"
        }))
        .unwrap();
        assert!(!actor.manually_approves_followers);
    }

    #[test]
    fn ap_actor_manually_approves_followers_reflects_true() {
        let actor: ApActor = serde_json::from_value(serde_json::json!({
            "id": "https://example.com/users/alice",
            "type": "Person",
            "manuallyApprovesFollowers": true
        }))
        .unwrap();
        assert!(actor.manually_approves_followers);
    }

    // ─── classify_ap_visibility ────────────────────────────────────────────

    #[test]
    fn classify_ap_visibility_public() {
        let to = vec!["https://www.w3.org/ns/activitystreams#Public".to_string()];
        let cc = vec!["https://example.com/users/alice/followers".to_string()];
        assert_eq!(classify_ap_visibility(&to, &cc), "public");
    }

    #[test]
    fn classify_ap_visibility_unlisted() {
        let to = vec!["https://example.com/users/alice/followers".to_string()];
        let cc = vec!["https://www.w3.org/ns/activitystreams#Public".to_string()];
        assert_eq!(classify_ap_visibility(&to, &cc), "unlisted");
    }

    #[test]
    fn classify_ap_visibility_followers_only() {
        let to = vec!["https://example.com/users/alice/followers".to_string()];
        let cc: Vec<String> = vec![];
        assert_eq!(classify_ap_visibility(&to, &cc), "followers_only");
    }

    #[test]
    fn classify_ap_visibility_direct() {
        let to = vec!["https://example.com/users/bob".to_string()];
        let cc: Vec<String> = vec![];
        assert_eq!(classify_ap_visibility(&to, &cc), "direct");
    }

    // ─── ApClient 公開鍵キャッシュのTTL ────────────────────────────────────

    #[tokio::test]
    async fn get_public_key_pem_returns_cached_value_when_fresh() {
        let client = ApClient::new(Arc::new(reqwest::Client::new()));
        {
            let mut cache = client.key_cache.write().await;
            cache.insert(
                "https://example.com/users/alice#main-key".to_string(),
                ("PEM-DATA".to_string(), Instant::now()),
            );
        }
        // TTL内のキャッシュヒットのため、ネットワークアクセスなしで即座に返る。
        let pem = client
            .get_public_key_pem("https://example.com/users/alice#main-key", None)
            .await
            .unwrap();
        assert_eq!(pem, "PEM-DATA");
    }

    #[tokio::test]
    async fn get_public_key_pem_ignores_stale_cache_entry() {
        let client = ApClient::new(Arc::new(reqwest::Client::new()));
        let stale_at = Instant::now()
            .checked_sub(KEY_CACHE_TTL + Duration::from_secs(1))
            .unwrap();
        {
            let mut cache = client.key_cache.write().await;
            cache.insert(
                "https://example.com/users/alice#main-key".to_string(),
                ("OLD-PEM".to_string(), stale_at),
            );
        }
        // TTL切れのためキャッシュを使わず再フェッチを試みる。到達不能ホストなのでエラーになるが、
        // 「古いPEMをそのまま返してしまう」ことがないのが検証したい点。
        let result = client
            .get_public_key_pem("https://127.0.0.1.invalid/users/alice#main-key", None)
            .await;
        assert!(result.is_err());
    }

    // ─── ApActor::also_known_as（Move受信の本人確認、docs/protocols.md参照） ────────

    fn minimal_actor_json(also_known_as: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": "https://newhost.example/users/alice",
            "type": "Person",
            "alsoKnownAs": also_known_as,
        })
    }

    #[test]
    fn also_known_as_accepts_array_form() {
        let actor: ApActor = serde_json::from_value(minimal_actor_json(serde_json::json!([
            "https://oldhost.example/users/alice",
            "https://third.example/users/alice"
        ])))
        .unwrap();
        assert!(actor.claims_also_known_as("https://oldhost.example/users/alice"));
        assert!(!actor.claims_also_known_as("https://unrelated.example/users/alice"));
    }

    #[test]
    fn also_known_as_accepts_single_string_form() {
        let actor: ApActor = serde_json::from_value(minimal_actor_json(serde_json::json!(
            "https://oldhost.example/users/alice"
        )))
        .unwrap();
        assert!(actor.claims_also_known_as("https://oldhost.example/users/alice"));
    }

    #[test]
    fn also_known_as_missing_field_defaults_to_empty() {
        let actor: ApActor = serde_json::from_value(serde_json::json!({
            "id": "https://newhost.example/users/alice",
            "type": "Person",
        }))
        .unwrap();
        assert!(!actor.claims_also_known_as("https://oldhost.example/users/alice"));
    }

    /// bridgy-fed は `featured` を URL 文字列ではなく OrderedCollection オブジェクトの
    /// インライン埋め込みで返す。この形が来ても Actor 全体のデシリアライズが失敗しないこと
    /// （旧来 `Option<String>` だった頃は "invalid type: map, expected a string" で失敗していた）。
    #[test]
    fn featured_accepts_inline_ordered_collection_object() {
        let actor: ApActor = serde_json::from_value(serde_json::json!({
            "id": "https://bsky.brid.gy/ap/did:plc:example",
            "type": "Person",
            "featured": {
                "id": "https://bsky.brid.gy/ap/did:plc:example/featured",
                "type": "OrderedCollection",
                "orderedItems": [],
            },
        }))
        .unwrap();
        assert!(actor.featured.unwrap().is_object());
    }

    #[test]
    fn featured_accepts_url_string_form() {
        let actor: ApActor = serde_json::from_value(serde_json::json!({
            "id": "https://mastodon.example/users/alice",
            "type": "Person",
            "featured": "https://mastodon.example/users/alice/collections/featured",
        }))
        .unwrap();
        assert_eq!(
            actor.featured.unwrap().as_str(),
            Some("https://mastodon.example/users/alice/collections/featured")
        );
    }

    // ─── verify_with_pem ────────────────────────────────────────────────

    /// Pleroma (post.syobon.net) が実際に返す publicKeyPem。末尾に空行が付く。
    const PLEROMA_STYLE_PEM_TRAILING_BLANK_LINE: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAsmQHhGDc7Yjutl+Grb7M\nHhW2dcGIA7uBjoYMJaH6B8CcgSz6+N+JmE+GvRrjugL4PGDdN6K6LMRiB7ih+1XB\nwYE1JUlTH+itsRfgwEvnfL9CM9Yfxabfc66QxiiJ7Kgenkh1I8j6gICulnwkZ89T\ncQHeM2va8qKw07yNz9tExPmjQFanPfYfUoeBnZwXVnTRiILLOu/vjqAq4avzGpG6\nSOXPAZMuWtIjMeqUWNivloo27voF3ZyzFFnXk1XMQJqXRv9Iik00pcBk7rZxMzrZ\nASsdOumltrygVjfx/LYh9vHosZRRcJUXT6N9NVudYshNWh1h469mCOTCfDe/2HO+\nFQIDAQAB\n-----END PUBLIC KEY-----\n\n";

    #[test]
    fn verify_with_pem_accepts_pleroma_trailing_blank_line() {
        // 末尾の空行があっても `RsaPublicKey::from_public_key_pem` がエラーにならない
        // ことだけを確認する（署名検証自体は失敗して構わない）。
        let result =
            ApClient::verify_with_pem(PLEROMA_STYLE_PEM_TRAILING_BLANK_LINE, "dummy", &[0u8; 32]);
        match result {
            Err(ApError::Signature(msg)) => {
                assert!(
                    !msg.contains("パース失敗"),
                    "PEMパース自体で失敗してはならない: {}",
                    msg
                );
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }
}
