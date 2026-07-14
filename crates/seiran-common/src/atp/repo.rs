//! AT Protocol PDS リポジトリ管理
//!
//! MST (Merkle Search Tree) の構築、commit の P-256 署名、CAR ファイルの生成、
//! および subscribeRepos WebSocket フレームの構築を担当する。

use argon2::password_hash::rand_core::{OsRng, RngCore};
pub use ipld_core::cid::Cid;
use ipld_core::ipld::Ipld;
use multihash::Multihash;
use p256::ecdsa::{signature::Signer, SigningKey};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use hex;

// ─────────────────────────────────────────────────────────────────────────────
// エラー型
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("CBOR エンコードエラー: {0}")]
    Cbor(String),
    #[error("鍵エラー: {0}")]
    Key(String),
    #[error("CID パースエラー: {0}")]
    CidParse(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// CID ヘルパー
// ─────────────────────────────────────────────────────────────────────────────

/// DAG-CBOR バイト列から CIDv1 (codec=dag-cbor, hash=sha2-256) を計算する。
pub fn cid_from_dagcbor(cbor: &[u8]) -> Cid {
    let hash = Sha256::digest(cbor);
    // SHA-256 multihash: code=0x12, digest_len=32
    let mh = Multihash::<64>::wrap(0x12, hash.as_slice()).expect("multihash wrap");
    Cid::new_v1(0x71, mh) // 0x71 = dag-cbor
}

/// CID を base32lower 文字列に変換する（例: "bafyrei..."）。
pub fn cid_to_string(cid: &Cid) -> String {
    cid.to_string()
}

/// CID 文字列をパースする。
pub fn cid_from_str(s: &str) -> Result<Cid, RepoError> {
    s.parse::<Cid>().map_err(|e| RepoError::CidParse(e.to_string()))
}

// ─────────────────────────────────────────────────────────────────────────────
// TID 生成（AT Protocol の rkey 形式）
// ─────────────────────────────────────────────────────────────────────────────
//
// TID = 53bit マイクロ秒タイムスタンプ || 10bit クロックID
// base32sortable 13 文字に変換（alphabet: 234567abcdefghijklmnopqrstuvwxyz）

const S32_CHARS: &[u8] = b"234567abcdefghijklmnopqrstuvwxyz";

pub fn generate_tid() -> String {
    let ts_us = chrono::Utc::now().timestamp_micros() as u64;
    let clock_id = (OsRng.next_u32() as u64) & 0x3FF;
    let value = (ts_us << 10) | clock_id;
    let mut chars = [0u8; 13];
    let mut v = value;
    for i in (0..13).rev() {
        chars[i] = S32_CHARS[(v & 0x1F) as usize];
        v >>= 5;
    }
    String::from_utf8(chars.to_vec()).expect("S32_CHARS は ASCII のみのため常に有効")
}

// ─────────────────────────────────────────────────────────────────────────────
// MST (Merkle Search Tree) 構築
// ─────────────────────────────────────────────────────────────────────────────
//
// AT Protocol MST の高さ（layer）は SHA-256(key) の base32lower（RFC 4648 lowercase）で
// 先頭の 'a'（= 0b00000 = 0）の個数によって決まる。
// 同じ layer のキーが同じノードに入り、layer の低いキーはサブツリーに降りる。

fn leading_zeros_on_hash(key: &str) -> u32 {
    let hash = Sha256::digest(key.as_bytes());
    let b32 = base32::encode(
        base32::Alphabet::RFC4648 { padding: false },
        hash.as_slice(),
    )
    .to_lowercase();
    b32.chars().take_while(|&c| c == 'a').count() as u32
}

fn common_prefix_len(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).take_while(|(ca, cb)| ca == cb).count()
}

// MST ノード（DAG-CBOR シリアライズ用）— canonical 順: e (0x65) < l (0x6c)
#[derive(Serialize)]
struct MstNode {
    e: Vec<MstEntry>,
    l: Option<Cid>,
}

// MST エントリ — canonical 順: k (0x6b) < p (0x70) < t (0x74) < v (0x76)
#[derive(Serialize)]
struct MstEntry {
    #[serde(with = "serde_bytes")]
    k: Vec<u8>,
    p: u32,
    t: Option<Cid>,
    v: Cid,
}

type Blocks = Vec<(Cid, Vec<u8>)>;

/// ソート済みの (key, record_cid) リストから MST を構築する。
/// (root_cid, blocks) を返す。
pub fn build_mst(entries: &[(String, Cid)]) -> Result<(Cid, Blocks), RepoError> {
    if entries.is_empty() {
        let node = MstNode { e: vec![], l: None };
        let cbor = serde_ipld_dagcbor::to_vec(&node).map_err(|e| RepoError::Cbor(e.to_string()))?;
        let cid = cid_from_dagcbor(&cbor);
        return Ok((cid, vec![(cid, cbor)]));
    }
    let layer = entries
        .iter()
        .map(|(k, _)| leading_zeros_on_hash(k))
        .max()
        .unwrap_or(0);
    build_layer(entries, layer)
}

fn build_layer(entries: &[(String, Cid)], layer: u32) -> Result<(Cid, Blocks), RepoError> {
    let mut blocks: Blocks = vec![];
    let mut node_entries: Vec<MstEntry> = vec![];
    let mut subtree_buf: Vec<(String, Cid)> = vec![];
    let mut left_subtree: Option<Cid> = None;
    let mut prev_key_bytes: Vec<u8> = vec![];

    let flush_subtree =
        |subtree_buf: &mut Vec<(String, Cid)>, blocks: &mut Blocks| -> Result<Option<Cid>, RepoError> {
            if subtree_buf.is_empty() {
                return Ok(None);
            }
            let (sc, sb) = build_layer(subtree_buf, layer.saturating_sub(1))?;
            blocks.extend(sb);
            subtree_buf.clear();
            Ok(Some(sc))
        };

    for (key, cid) in entries {
        let h = leading_zeros_on_hash(key);
        let key_bytes = key.as_bytes();

        if h < layer {
            subtree_buf.push((key.clone(), *cid));
        } else {
            // h == layer
            let sc = flush_subtree(&mut subtree_buf, &mut blocks)?;
            if node_entries.is_empty() {
                left_subtree = sc;
            } else {
                node_entries.last_mut().unwrap().t = sc;
            }

            let prefix_len = common_prefix_len(&prev_key_bytes, key_bytes);
            prev_key_bytes = key_bytes.to_vec();

            node_entries.push(MstEntry {
                p: prefix_len as u32,
                k: key_bytes[prefix_len..].to_vec(),
                v: *cid,
                t: None,
            });
        }
    }

    // 末尾のサブツリーを処理
    if !subtree_buf.is_empty() {
        let (sc, sb) = build_layer(&subtree_buf, layer.saturating_sub(1))?;
        blocks.extend(sb);
        if node_entries.is_empty() {
            left_subtree = Some(sc);
        } else {
            node_entries.last_mut().unwrap().t = Some(sc);
        }
    }

    let node = MstNode {
        e: node_entries,
        l: left_subtree,
    };
    let cbor = serde_ipld_dagcbor::to_vec(&node).map_err(|e| RepoError::Cbor(e.to_string()))?;
    let cid = cid_from_dagcbor(&cbor);
    blocks.push((cid, cbor));

    Ok((cid, blocks))
}

// ─────────────────────────────────────────────────────────────────────────────
// Commit 生成と P-256 署名
// ─────────────────────────────────────────────────────────────────────────────

// 未署名 commit
// canonical 順: did(3) < rev(3) < data(4) < prev(4) < version(7)
// ただし同長は辞書順: did < rev ('d' < 'r'), data < prev ('d' < 'p')
#[derive(Serialize)]
struct UnsignedCommit {
    data: Cid,
    did: String,
    prev: Option<Cid>,
    rev: String,
    version: u64,
}

// 署名済み commit (sig を追加)
// canonical 順: did(3) < rev(3) < sig(3) < data(4) < prev(4) < version(7)
// 同長 3: did < rev < sig ('d' < 'r' < 's')
#[derive(Serialize)]
struct SignedCommit {
    data: Cid,
    did: String,
    prev: Option<Cid>,
    rev: String,
    #[serde(with = "serde_bytes")]
    sig: Vec<u8>,
    version: u64,
}

/// commit を生成して P-256 で署名し、(commit_cid, commit_cbor) を返す。
///
/// - `signing_key`: actors.at_signing_key_pem から復元したユーザー固有の鍵
/// - 署名は未署名 commit の DAG-CBOR バイト列に対して行われる（内部で SHA-256 適用）
pub fn create_commit(
    did: &str,
    rev: &str,
    mst_root: Cid,
    prev: Option<Cid>,
    signing_key: &SigningKey,
) -> Result<(Cid, Vec<u8>), RepoError> {
    let unsigned = UnsignedCommit {
        data: mst_root,
        did: did.to_string(),
        prev,
        rev: rev.to_string(),
        version: 3,
    };
    let unsigned_cbor =
        serde_ipld_dagcbor::to_vec(&unsigned).map_err(|e| RepoError::Cbor(e.to_string()))?;

    // p256 Signer::sign は内部で SHA-256 を適用する
    let sig: p256::ecdsa::Signature = signing_key.sign(&unsigned_cbor);
    let sig_bytes = sig.to_bytes().to_vec(); // IEEE P1363: R(32) || S(32) = 64 bytes

    let signed = SignedCommit {
        data: mst_root,
        did: did.to_string(),
        prev,
        rev: rev.to_string(),
        sig: sig_bytes,
        version: 3,
    };
    let commit_cbor =
        serde_ipld_dagcbor::to_vec(&signed).map_err(|e| RepoError::Cbor(e.to_string()))?;
    let commit_cid = cid_from_dagcbor(&commit_cbor);

    Ok((commit_cid, commit_cbor))
}

// ─────────────────────────────────────────────────────────────────────────────
// CAR ファイルエンコーダ (CARv1)
// ─────────────────────────────────────────────────────────────────────────────
//
// フォーマット:
//   varint(header_len) + header_cbor
//   [varint(cid_len + block_len) + cid_raw_bytes + block_bytes] * n
//
// header_cbor = {"roots": [commit_cid], "version": 1}
// cid_raw_bytes = CIDv1 のバイト列（multibase prefix なし）

pub fn encode_car(root_cid: &Cid, blocks: &[(Cid, Vec<u8>)]) -> Result<Vec<u8>, RepoError> {
    // ヘッダー: {"roots": [CID], "version": 1}
    // canonical 順: roots(5) < version(7)
    let mut header_map: BTreeMap<String, Ipld> = BTreeMap::new();
    header_map.insert(
        "roots".to_string(),
        Ipld::List(vec![Ipld::Link(*root_cid)]),
    );
    header_map.insert("version".to_string(), Ipld::Integer(1));
    let header_cbor = serde_ipld_dagcbor::to_vec(&Ipld::Map(header_map))
        .map_err(|e| RepoError::Cbor(e.to_string()))?;

    let mut car = vec![];
    encode_uvarint(header_cbor.len() as u64, &mut car);
    car.extend_from_slice(&header_cbor);

    for (cid, block) in blocks {
        let cid_bytes = cid.to_bytes(); // raw CID bytes (no multibase prefix)
        let total = cid_bytes.len() + block.len();
        encode_uvarint(total as u64, &mut car);
        car.extend_from_slice(&cid_bytes);
        car.extend_from_slice(block);
    }

    Ok(car)
}

fn encode_uvarint(mut n: u64, buf: &mut Vec<u8>) {
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// app.bsky.feed.post レコード（Facet 型を含む）
// ─────────────────────────────────────────────────────────────────────────────

/// AT Protocol Facet の index（UTF-8 バイトオフセット）
/// canonical 順: byteEnd(7) < byteStart(9)
#[derive(Serialize)]
pub struct BskyFacetIndex {
    #[serde(rename = "byteEnd")]
    pub byte_end: usize,
    #[serde(rename = "byteStart")]
    pub byte_start: usize,
}

/// Facet feature: mention
/// canonical 順: did(3) < $type(5)
#[derive(Serialize)]
pub struct BskyFacetMention {
    pub did: String,
    #[serde(rename = "$type")]
    pub kind: String,
}

/// AT Protocol リッチテキスト Facet（`app.bsky.richtext.facet`）
/// canonical 順: index(5) < features(8)
#[derive(Serialize)]
pub struct BskyFacet {
    pub index: BskyFacetIndex,
    pub features: Vec<BskyFacetMention>,
}

/// 画像添付情報（`app.bsky.embed.images` 生成用）
pub struct BskyImage {
    /// 保存済みバイナリの SHA-256（hex）— raw CIDv1 の生成に使用
    pub sha256_hex: String,
    pub mime_type: String,
    pub size: i64,
    pub width: i32,
    pub height: i32,
    pub alt: String,
}

/// SHA-256 ハッシュ（hex 文字列）から CIDv1 (raw codec=0x55) を生成する。
/// AT Protocol Blob 参照で使用する。
pub fn cid_from_sha256_hex(sha256_hex: &str) -> Result<Cid, RepoError> {
    let bytes = hex::decode(sha256_hex)
        .map_err(|e| RepoError::CidParse(format!("SHA-256 hex デコード失敗: {}", e)))?;
    let mh = Multihash::<64>::wrap(0x12, &bytes)
        .map_err(|e| RepoError::CidParse(format!("multihash wrap 失敗: {}", e)))?;
    Ok(Cid::new_v1(0x55, mh))
}

// ─────────────────────────────────────────────────────────────────────────────
// app.bsky.feed.post の reply フィールド型
// ─────────────────────────────────────────────────────────────────────────────

/// AT Protocol ポスト参照（uri + cid）。
/// canonical 順: cid(3) < uri(3) → 同長は辞書順: c < u
#[derive(Serialize)]
pub struct BskyRefRecord {
    pub cid: String,
    pub uri: String,
}

/// app.bsky.feed.post の reply フィールド。
/// canonical 順: root(4) < parent(6)
#[derive(Serialize)]
pub struct BskyPostReply {
    pub root: BskyRefRecord,
    pub parent: BskyRefRecord,
}

// ─────────────────────────────────────────────────────────────────────────────
// app.bsky.feed.post レコード構造体
// ─────────────────────────────────────────────────────────────────────────────

// canonical 順: text(4) < $type(5) < embed(5) < reply(5) < facets(6) < createdAt(9)
// 5文字のキー同士: "$type"($ = 0x24) < "embed"(e = 0x65) < "reply"(r = 0x72)
#[derive(Serialize)]
struct BskyFeedPost {
    text: String,
    #[serde(rename = "$type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    embed: Option<Ipld>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<BskyPostReply>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    facets: Vec<BskyFacet>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// app.bsky.feed.repost レコード構造体
// ─────────────────────────────────────────────────────────────────────────────

// canonical 順: $type(5) < subject(7) < createdAt(9)
#[derive(Serialize)]
struct BskyFeedRepost {
    #[serde(rename = "$type")]
    kind: String,
    subject: BskyRefRecord,
    #[serde(rename = "createdAt")]
    created_at: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// app.bsky.feed.like レコード構造体（リアクション連携）
// ─────────────────────────────────────────────────────────────────────────────

/// `app.bsky.feed.like` レコード。ATP には絵文字リアクションの概念が無いため、
/// どの絵文字であっても Like として送る。`emoji` は非標準の拡張フィールド
/// （seiran 独自。公式 Bluesky クライアントは無視するだけのはず）。
#[derive(Serialize)]
struct BskyFeedLike {
    #[serde(rename = "$type")]
    kind: String,
    subject: BskyRefRecord,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    emoji: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Bsky embed 種別
// ─────────────────────────────────────────────────────────────────────────────

/// AT Protocol ポスト embed の種別。
pub enum BskyEmbed {
    Images(Vec<BskyImage>),
    Record { uri: String, cid: String },
    External { url: String },
    /// Bsky公式動画パイプライン（`app.bsky.video.uploadVideo`）で発行された
    /// blob CIDを使った動画embed。`bsky_video_status='ready'`の場合のみ使用する。
    Video { cid: String, mime_type: String, size: i64, width: i32, height: i32 },
}

/// `app.bsky.embed.record`（引用ポスト）の Ipld を構築する。
fn build_embed_record_ipld(uri: &str, cid_str: &str) -> Ipld {
    let mut record = BTreeMap::new();
    record.insert("cid".to_string(), Ipld::String(cid_str.to_string()));
    record.insert("uri".to_string(), Ipld::String(uri.to_string()));

    let mut embed = BTreeMap::new();
    embed.insert("$type".to_string(), Ipld::String("app.bsky.embed.record".to_string()));
    embed.insert("record".to_string(), Ipld::Map(record));
    Ipld::Map(embed)
}

/// `app.bsky.embed.external`（URL カード）の Ipld を構築する。
fn build_embed_external_ipld(url: &str) -> Ipld {
    let mut external = BTreeMap::new();
    external.insert("description".to_string(), Ipld::String(String::new()));
    external.insert("title".to_string(), Ipld::String(String::new()));
    external.insert("uri".to_string(), Ipld::String(url.to_string()));

    let mut embed = BTreeMap::new();
    embed.insert("$type".to_string(), Ipld::String("app.bsky.embed.external".to_string()));
    embed.insert("external".to_string(), Ipld::Map(external));
    Ipld::Map(embed)
}

/// AT Protocol の `blob` 参照 Ipld を構築する（`app.bsky.embed.images` の画像・
/// `app.bsky.actor.profile` の avatar など、blob 参照を持つ全レコードで共有する）。
///
/// blob: canonical 順 ref(3) < size(4) < $type(5) < mimeType(8)
/// ref は CID リンク（DAG-CBOR tag 42）を直接持つ
fn build_blob_ipld(sha256_hex: &str, mime_type: &str, size: i64) -> Result<Ipld, RepoError> {
    let blob_cid = cid_from_sha256_hex(sha256_hex)?;
    build_blob_ipld_from_cid_value(blob_cid, mime_type, size)
}

/// `build_blob_ipld`と同じblob参照Ipldを、sha256からではなく既知のCID文字列から
/// 直接構築する（Bsky公式動画パイプラインが発行したCIDのように、seiran側で
/// バイナリを保持していない/sha256から逆算できないケース向け）。
fn build_blob_ipld_from_cid(cid_str: &str, mime_type: &str, size: i64) -> Result<Ipld, RepoError> {
    let blob_cid = cid_from_str(cid_str)?;
    build_blob_ipld_from_cid_value(blob_cid, mime_type, size)
}

fn build_blob_ipld_from_cid_value(blob_cid: Cid, mime_type: &str, size: i64) -> Result<Ipld, RepoError> {
    let mut blob_map = BTreeMap::new();
    blob_map.insert("ref".to_string(), Ipld::Link(blob_cid));
    blob_map.insert("size".to_string(), Ipld::Integer(size as i128));
    blob_map.insert("$type".to_string(), Ipld::String("blob".to_string()));
    blob_map.insert("mimeType".to_string(), Ipld::String(mime_type.to_string()));
    Ok(Ipld::Map(blob_map))
}

/// `app.bsky.embed.video` の Ipld ツリーを構築する。
fn build_embed_video_ipld(cid_str: &str, mime_type: &str, size: i64, width: i32, height: i32) -> Result<Ipld, RepoError> {
    let blob_map = build_blob_ipld_from_cid(cid_str, mime_type, size)?;

    let mut embed = BTreeMap::new();
    embed.insert("alt".to_string(), Ipld::String(String::new()));
    embed.insert("$type".to_string(), Ipld::String("app.bsky.embed.video".to_string()));
    embed.insert("video".to_string(), blob_map);
    if width > 0 && height > 0 {
        let mut aspect = BTreeMap::new();
        aspect.insert("width".to_string(), Ipld::Integer(width as i128));
        aspect.insert("height".to_string(), Ipld::Integer(height as i128));
        embed.insert("aspectRatio".to_string(), Ipld::Map(aspect));
    }
    Ok(Ipld::Map(embed))
}

/// `app.bsky.embed.images` の Ipld ツリーを構築する。
fn build_embed_images_ipld(images: &[BskyImage]) -> Result<Ipld, RepoError> {
    let image_list: Result<Vec<Ipld>, RepoError> = images.iter().map(|img| {
        let blob_map = build_blob_ipld(&img.sha256_hex, &img.mime_type, img.size)?;

        // aspectRatio: canonical 順 width(5) < height(6)
        let mut aspect = BTreeMap::new();
        aspect.insert("width".to_string(), Ipld::Integer(img.width as i128));
        aspect.insert("height".to_string(), Ipld::Integer(img.height as i128));

        // image item: canonical 順 alt(3) < image(5) < aspectRatio(11)
        let mut item = BTreeMap::new();
        item.insert("alt".to_string(), Ipld::String(img.alt.clone()));
        item.insert("image".to_string(), blob_map);
        item.insert("aspectRatio".to_string(), Ipld::Map(aspect));

        Ok(Ipld::Map(item))
    }).collect();

    // embed: canonical 順 $type(5) < images(6)
    let mut embed = BTreeMap::new();
    embed.insert("$type".to_string(), Ipld::String("app.bsky.embed.images".to_string()));
    embed.insert("images".to_string(), Ipld::List(image_list?));

    Ok(Ipld::Map(embed))
}

/// `app.bsky.feed.post` レコードの DAG-CBOR バイト列と CID を生成する。
///
/// `facets` が空の場合は `facets` フィールドを省略する。
/// `embed` が Some の場合は embed フィールドを含める（画像・引用・URL カード）。
/// `reply` が Some の場合は `reply` フィールドを含める（リプライ投稿）。
pub fn encode_bsky_feed_post(
    text: &str,
    created_at_rfc3339: &str,
    facets: Vec<BskyFacet>,
    embed: Option<BskyEmbed>,
    reply: Option<BskyPostReply>,
) -> Result<(Vec<u8>, Cid), RepoError> {
    let embed = match embed {
        None => None,
        Some(BskyEmbed::Images(images)) => {
            if images.is_empty() { None } else { Some(build_embed_images_ipld(&images)?) }
        }
        Some(BskyEmbed::Record { uri, cid }) => Some(build_embed_record_ipld(&uri, &cid)),
        Some(BskyEmbed::External { url }) => Some(build_embed_external_ipld(&url)),
        Some(BskyEmbed::Video { cid, mime_type, size, width, height }) => {
            Some(build_embed_video_ipld(&cid, &mime_type, size, width, height)?)
        }
    };
    let record = BskyFeedPost {
        text: text.to_string(),
        kind: "app.bsky.feed.post".to_string(),
        embed,
        reply,
        facets,
        created_at: created_at_rfc3339.to_string(),
    };
    let cbor = serde_ipld_dagcbor::to_vec(&record).map_err(|e| RepoError::Cbor(e.to_string()))?;
    let cid = cid_from_dagcbor(&cbor);
    Ok((cbor, cid))
}

/// `app.bsky.feed.repost` レコードの DAG-CBOR バイト列と CID を生成する。
pub fn encode_bsky_feed_repost(
    at_uri: &str,
    at_cid: &str,
    created_at_rfc3339: &str,
) -> Result<(Vec<u8>, Cid), RepoError> {
    let record = BskyFeedRepost {
        kind: "app.bsky.feed.repost".to_string(),
        subject: BskyRefRecord {
            cid: at_cid.to_string(),
            uri: at_uri.to_string(),
        },
        created_at: created_at_rfc3339.to_string(),
    };
    let cbor = serde_ipld_dagcbor::to_vec(&record).map_err(|e| RepoError::Cbor(e.to_string()))?;
    let cid = cid_from_dagcbor(&cbor);
    Ok((cbor, cid))
}

/// `app.bsky.feed.like` レコードの DAG-CBOR バイト列と CID を生成する。
/// `emoji` は非標準の拡張フィールド（Some の場合のみレコードへ含める）。
pub fn encode_bsky_feed_like(
    at_uri: &str,
    at_cid: &str,
    created_at_rfc3339: &str,
    emoji: Option<&str>,
) -> Result<(Vec<u8>, Cid), RepoError> {
    let record = BskyFeedLike {
        kind: "app.bsky.feed.like".to_string(),
        subject: BskyRefRecord {
            cid: at_cid.to_string(),
            uri: at_uri.to_string(),
        },
        created_at: created_at_rfc3339.to_string(),
        emoji: emoji.map(|s| s.to_string()),
    };
    let cbor = serde_ipld_dagcbor::to_vec(&record).map_err(|e| RepoError::Cbor(e.to_string()))?;
    let cid = cid_from_dagcbor(&cbor);
    Ok((cbor, cid))
}

/// `app.bsky.actor.profile` レコードの DAG-CBOR バイト列と CID を生成する。
///
/// `description` が Some の場合は bio を、`avatar` が Some の場合は
/// アイコン画像の blob 参照（sha256_hex, mime_type, size）を含める。
pub fn encode_bsky_actor_profile(
    display_name: &str,
    description: Option<&str>,
    avatar: Option<(&str, &str, i64)>,
    created_at_rfc3339: &str,
) -> Result<(Vec<u8>, Cid), RepoError> {
    // canonical 順: $type(5) < avatar(6) < createdAt(9) < description(11) < displayName(11)
    // 11文字キー同士: "description"(e=0x65) < "displayName"(i=0x69)
    #[derive(Serialize)]
    struct BskyActorProfile {
        #[serde(rename = "$type")]
        kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        avatar: Option<Ipld>,
        #[serde(rename = "createdAt")]
        created_at: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(rename = "displayName")]
        display_name: String,
    }
    let avatar_ipld = match avatar {
        Some((sha256_hex, mime_type, size)) => Some(build_blob_ipld(sha256_hex, mime_type, size)?),
        None => None,
    };
    let record = BskyActorProfile {
        kind: "app.bsky.actor.profile".to_string(),
        avatar: avatar_ipld,
        created_at: created_at_rfc3339.to_string(),
        description: description.map(|s| s.to_string()),
        display_name: display_name.to_string(),
    };
    let cbor = serde_ipld_dagcbor::to_vec(&record).map_err(|e| RepoError::Cbor(e.to_string()))?;
    let cid = cid_from_dagcbor(&cbor);
    Ok((cbor, cid))
}

/// `app.bsky.graph.follow` レコードの DAG-CBOR バイト列と CID を生成する。
pub fn encode_bsky_graph_follow(
    subject_did: &str,
    created_at_rfc3339: &str,
) -> Result<(Vec<u8>, Cid), RepoError> {
    // canonical 順: $type(5) < subject(7) < createdAt(9)
    #[derive(Serialize)]
    struct BskyGraphFollow {
        #[serde(rename = "$type")]
        kind: String,
        subject: String,
        #[serde(rename = "createdAt")]
        created_at: String,
    }
    let record = BskyGraphFollow {
        kind: "app.bsky.graph.follow".to_string(),
        subject: subject_did.to_string(),
        created_at: created_at_rfc3339.to_string(),
    };
    let cbor = serde_ipld_dagcbor::to_vec(&record).map_err(|e| RepoError::Cbor(e.to_string()))?;
    let cid = cid_from_dagcbor(&cbor);
    Ok((cbor, cid))
}

// ─────────────────────────────────────────────────────────────────────────────
// subscribeRepos WebSocket フレーム構築
// ─────────────────────────────────────────────────────────────────────────────
//
// 各 WebSocket バイナリフレーム = cbor(header) + cbor(body) （連結、区切りなし）
//
// header: {"op": 1, "t": "#commit"}
// body:   CommitEvt 構造体

pub struct CommitEvtOp {
    pub action: String,      // "create" | "update" | "delete"
    pub path: String,        // "app.bsky.feed.post/<tid>"
    pub cid: Option<Cid>,   // delete の場合は None
}

#[allow(clippy::too_many_arguments)]
pub fn build_commit_frame(
    seq: i64,
    did: &str,
    commit_cid: &Cid,
    prev_cid: Option<&Cid>,
    rev: &str,
    since: Option<&str>,
    car_bytes: &[u8],
    ops: &[CommitEvtOp],
    blob_cids: &[Cid],
    time: &str,
) -> Result<Vec<u8>, RepoError> {
    // ヘッダー CBOR
    // canonical 順: op(2) < t(1)... wait: "op"(2) vs "t"(1)
    // length 1: "t" → comes first
    // length 2: "op" → comes second
    let mut header_map: BTreeMap<String, Ipld> = BTreeMap::new();
    header_map.insert("op".to_string(), Ipld::Integer(1));
    header_map.insert("t".to_string(), Ipld::String("#commit".to_string()));
    let header_cbor = serde_ipld_dagcbor::to_vec(&Ipld::Map(header_map))
        .map_err(|e| RepoError::Cbor(e.to_string()))?;

    // ボディ CBOR（Ipld::Map で canonical ordering は自動処理）
    let ops_ipld: Vec<Ipld> = ops
        .iter()
        .map(|op| {
            let mut m: BTreeMap<String, Ipld> = BTreeMap::new();
            m.insert("action".to_string(), Ipld::String(op.action.clone()));
            m.insert("cid".to_string(), match op.cid {
                Some(c) => Ipld::Link(c),
                None => Ipld::Null,
            });
            m.insert("path".to_string(), Ipld::String(op.path.clone()));
            Ipld::Map(m)
        })
        .collect();

    let mut body_map: BTreeMap<String, Ipld> = BTreeMap::new();
    let blobs_ipld: Vec<Ipld> = blob_cids.iter().map(|c| Ipld::Link(*c)).collect();
    body_map.insert("blobs".to_string(), Ipld::List(blobs_ipld));
    body_map.insert("blocks".to_string(), Ipld::Bytes(car_bytes.to_vec()));
    body_map.insert("commit".to_string(), Ipld::Link(*commit_cid));
    body_map.insert("did".to_string(), Ipld::String(did.to_string()));
    body_map.insert("ops".to_string(), Ipld::List(ops_ipld));
    if let Some(c) = prev_cid {
        body_map.insert("prev".to_string(), Ipld::Link(*c));
    }
    body_map.insert("rebase".to_string(), Ipld::Bool(false));
    body_map.insert("repo".to_string(), Ipld::String(did.to_string()));
    body_map.insert("rev".to_string(), Ipld::String(rev.to_string()));
    body_map.insert("seq".to_string(), Ipld::Integer(seq as i128));
    if let Some(s) = since {
        body_map.insert("since".to_string(), Ipld::String(s.to_string()));
    }
    body_map.insert("time".to_string(), Ipld::String(time.to_string()));
    body_map.insert("tooBig".to_string(), Ipld::Bool(false));

    let body_cbor = serde_ipld_dagcbor::to_vec(&Ipld::Map(body_map))
        .map_err(|e| RepoError::Cbor(e.to_string()))?;

    let mut frame = header_cbor;
    frame.extend_from_slice(&body_cbor);
    Ok(frame)
}

/// subscribeRepos の #identity フレームを生成する。
/// handle が確定したとき AppView に再検証を促すために送信する。
pub fn build_identity_frame(seq: i64, did: &str, handle: &str, time: &str) -> Result<Vec<u8>, RepoError> {
    let mut header_map: BTreeMap<String, Ipld> = BTreeMap::new();
    header_map.insert("op".to_string(), Ipld::Integer(1));
    header_map.insert("t".to_string(), Ipld::String("#identity".to_string()));
    let header_cbor = serde_ipld_dagcbor::to_vec(&Ipld::Map(header_map))
        .map_err(|e| RepoError::Cbor(e.to_string()))?;

    let mut body_map: BTreeMap<String, Ipld> = BTreeMap::new();
    body_map.insert("did".to_string(), Ipld::String(did.to_string()));
    body_map.insert("handle".to_string(), Ipld::String(handle.to_string()));
    body_map.insert("seq".to_string(), Ipld::Integer(seq as i128));
    body_map.insert("time".to_string(), Ipld::String(time.to_string()));
    let body_cbor = serde_ipld_dagcbor::to_vec(&Ipld::Map(body_map))
        .map_err(|e| RepoError::Cbor(e.to_string()))?;

    let mut frame = header_cbor;
    frame.extend_from_slice(&body_cbor);
    Ok(frame)
}

/// subscribeRepos の #account フレームを生成する。
/// `active=false, status="deleted"` でアカウント削除を AppView/Relay に通知する。
pub fn build_account_frame(
    seq: i64,
    did: &str,
    handle: &str,
    time: &str,
    active: bool,
    status: Option<&str>,
) -> Result<Vec<u8>, RepoError> {
    let mut header_map: BTreeMap<String, Ipld> = BTreeMap::new();
    header_map.insert("op".to_string(), Ipld::Integer(1));
    header_map.insert("t".to_string(), Ipld::String("#account".to_string()));
    let header_cbor = serde_ipld_dagcbor::to_vec(&Ipld::Map(header_map))
        .map_err(|e| RepoError::Cbor(e.to_string()))?;

    let mut body_map: BTreeMap<String, Ipld> = BTreeMap::new();
    body_map.insert("active".to_string(), Ipld::Bool(active));
    body_map.insert("did".to_string(), Ipld::String(did.to_string()));
    body_map.insert("handle".to_string(), Ipld::String(handle.to_string()));
    body_map.insert("seq".to_string(), Ipld::Integer(seq as i128));
    if let Some(s) = status {
        body_map.insert("status".to_string(), Ipld::String(s.to_string()));
    }
    body_map.insert("time".to_string(), Ipld::String(time.to_string()));
    let body_cbor = serde_ipld_dagcbor::to_vec(&Ipld::Map(body_map))
        .map_err(|e| RepoError::Cbor(e.to_string()))?;

    let mut frame = header_cbor;
    frame.extend_from_slice(&body_cbor);
    Ok(frame)
}

/// subscribeRepos の #error フレームを生成する。
pub fn build_error_frame(name: &str, message: &str) -> Result<Vec<u8>, RepoError> {
    let mut header_map: BTreeMap<String, Ipld> = BTreeMap::new();
    header_map.insert("op".to_string(), Ipld::Integer(-1));
    header_map.insert("t".to_string(), Ipld::String("#error".to_string()));
    let header_cbor = serde_ipld_dagcbor::to_vec(&Ipld::Map(header_map))
        .map_err(|e| RepoError::Cbor(e.to_string()))?;

    let mut body_map: BTreeMap<String, Ipld> = BTreeMap::new();
    body_map.insert("message".to_string(), Ipld::String(message.to_string()));
    body_map.insert("name".to_string(), Ipld::String(name.to_string()));
    let body_cbor = serde_ipld_dagcbor::to_vec(&Ipld::Map(body_map))
        .map_err(|e| RepoError::Cbor(e.to_string()))?;

    let mut frame = header_cbor;
    frame.extend_from_slice(&body_cbor);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_tid_length() {
        let tid = generate_tid();
        assert_eq!(tid.len(), 13);
    }

    #[test]
    fn test_generate_tid_alphabet() {
        let tid = generate_tid();
        let valid: std::collections::HashSet<char> =
            "234567abcdefghijklmnopqrstuvwxyz".chars().collect();
        assert!(tid.chars().all(|c| valid.contains(&c)), "TID に無効な文字: {}", tid);
    }

    #[test]
    fn test_cid_roundtrip() {
        let cbor = b"test data";
        let cid = cid_from_dagcbor(cbor);
        let s = cid_to_string(&cid);
        let parsed = cid_from_str(&s).unwrap();
        assert_eq!(cid, parsed);
    }

    #[test]
    fn test_encode_bsky_feed_post_deterministic() {
        let (cbor1, cid1) = encode_bsky_feed_post("hello", "2024-01-01T00:00:00.000Z", vec![], None, None).unwrap();
        let (cbor2, cid2) = encode_bsky_feed_post("hello", "2024-01-01T00:00:00.000Z", vec![], None, None).unwrap();
        assert_eq!(cbor1, cbor2);
        assert_eq!(cid1, cid2);
    }

    #[test]
    fn test_build_mst_empty() {
        let (root, blocks) = build_mst(&[]).unwrap();
        assert!(!blocks.is_empty());
        assert_eq!(blocks[0].0, root);
    }

    #[test]
    fn test_build_mst_single_entry() {
        let (_, cid) = encode_bsky_feed_post("hi", "2024-01-01T00:00:00.000Z", vec![], None, None).unwrap();
        let entries = vec![("app.bsky.feed.post/test123".to_string(), cid)];
        let result = build_mst(&entries);
        assert!(result.is_ok());
    }

    #[test]
    fn test_encode_bsky_feed_like_with_emoji() {
        let (cbor, cid) = encode_bsky_feed_like(
            "at://did:plc:abc/app.bsky.feed.post/xyz",
            "bafyreidummycid",
            "2024-01-01T00:00:00.000Z",
            Some("👍"),
        ).unwrap();
        assert!(!cbor.is_empty());
        assert_eq!(cid_from_dagcbor(&cbor), cid);
    }

    #[test]
    fn test_encode_bsky_feed_like_without_emoji_omits_field() {
        let (with_emoji, _) = encode_bsky_feed_like("at://a/b/c", "cid1", "2024-01-01T00:00:00.000Z", Some("❤")).unwrap();
        let (without_emoji, _) = encode_bsky_feed_like("at://a/b/c", "cid1", "2024-01-01T00:00:00.000Z", None).unwrap();
        assert_ne!(with_emoji, without_emoji, "emoji フィールドの有無で CBOR が変わらないのはおかしい");
    }
}
