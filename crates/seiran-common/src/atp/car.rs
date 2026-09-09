//! CARv1 コンテナのデコード
//!
//! `atp/repo.rs` の `encode_car` の逆方向。他PDSから取得した `com.atproto.sync.getRepo`
//! のレスポンス（信頼できない外部入力）を読むため、サイズ・ネスト上限による
//! DoS対策を必須とする。
//!
//! フォーマット（`repo.rs::encode_car` のコメント参照）:
//!   varint(header_len) + header_cbor
//!   [varint(cid_len + block_len) + cid_raw_bytes + block_bytes] * n
//!
//! header_cbor = {"roots": [commit_cid], "version": 1}

use ipld_core::cid::Cid;
use ipld_core::ipld::Ipld;
use std::collections::HashMap;
use std::io::Cursor;

#[derive(Debug, thiserror::Error)]
pub enum CarError {
    #[error("CARファイルが空です")]
    Empty,
    #[error("uvarintのデコードに失敗しました（不正な形式）")]
    InvalidVarint,
    #[error("CARファイルが途中で切れています")]
    Truncated,
    #[error("CARファイルが上限サイズ（{0}バイト）を超えています")]
    TooLarge(usize),
    #[error("ブロックが上限サイズ（{0}バイト）を超えています")]
    BlockTooLarge(usize),
    #[error("ヘッダーのデコードに失敗しました: {0}")]
    Header(String),
    #[error("CIDのデコードに失敗しました: {0}")]
    Cid(String),

    // MST ウォーク側でも使うため、CAR コンテナのエラーと同居させる
    #[error("MSTノードのデコードに失敗しました: {0}")]
    Node(String),
    #[error("参照先ブロックが見つかりません（cid={0}）")]
    MissingBlock(String),
    #[error("MSTの深さが上限（{0}）を超えています")]
    TooDeep(usize),
    #[error("レコードキーがUTF-8として不正です")]
    InvalidKey,
    #[error("レコードキーの形式が不正です（collection/rkeyの形になっていない）: {0}")]
    MalformedKey(String),
}

/// CAR全体の上限（bsky.socialの`blobUploadLimit`実測値 314572800 に余裕を持たせた程度）。
const MAX_CAR_SIZE: usize = 512 * 1024 * 1024;
/// 個々のブロック（MSTノード or 1レコード）の上限。通常は数KB〜数百KBに収まる。
const MAX_BLOCK_SIZE: usize = 16 * 1024 * 1024;
/// ヘッダーCBORの上限（roots配列1件程度なので極小のはず）。
const MAX_HEADER_SIZE: usize = 1024 * 1024;
/// MSTウォークの再帰深さ上限（通常のアカウント規模なら数十で収まる）。
pub const MAX_MST_DEPTH: usize = 128;

pub struct CarFile {
    pub roots: Vec<Cid>,
    /// (cid, bytes) の到着順リスト。ルックアップ用の HashMap 化は呼び出し側で行う。
    pub blocks: Vec<(Cid, Vec<u8>)>,
}

fn read_uvarint(data: &[u8], pos: &mut usize) -> Result<u64, CarError> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(*pos).ok_or(CarError::Truncated)?;
        *pos += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 64 {
            return Err(CarError::InvalidVarint);
        }
    }
}

/// header_cbor の `Ipld::Map` から `roots` フィールド（CIDリンクの配列）を取り出す。
/// `Cid` 自体の `serde::Deserialize` 実装の有無に依存しないよう、`encode_car` の
/// エンコード側（`Ipld::Link` を直接組み立てる方式）と対称に、`Ipld` 経由でデコードする。
fn extract_roots(header: &Ipld) -> Result<Vec<Cid>, CarError> {
    let Ipld::Map(map) = header else {
        return Err(CarError::Header("ヘッダーがMap型ではありません".into()));
    };
    let Some(Ipld::List(items)) = map.get("roots") else {
        return Err(CarError::Header("rootsフィールドがありません".into()));
    };
    items
        .iter()
        .map(|item| match item {
            Ipld::Link(cid) => Ok(*cid),
            _ => Err(CarError::Header(
                "roots要素がCIDリンクではありません".into(),
            )),
        })
        .collect()
}

pub fn decode_car(data: &[u8]) -> Result<CarFile, CarError> {
    if data.len() > MAX_CAR_SIZE {
        return Err(CarError::TooLarge(MAX_CAR_SIZE));
    }
    if data.is_empty() {
        return Err(CarError::Empty);
    }

    let mut pos = 0usize;

    let header_len = read_uvarint(data, &mut pos)? as usize;
    if header_len > MAX_HEADER_SIZE {
        return Err(CarError::BlockTooLarge(MAX_HEADER_SIZE));
    }
    let header_end = pos.checked_add(header_len).ok_or(CarError::Truncated)?;
    let header_bytes = data.get(pos..header_end).ok_or(CarError::Truncated)?;
    pos = header_end;

    let header_ipld: Ipld = serde_ipld_dagcbor::from_slice(header_bytes)
        .map_err(|e| CarError::Header(e.to_string()))?;
    let roots = extract_roots(&header_ipld)?;

    let mut blocks = Vec::new();
    while pos < data.len() {
        let entry_len = read_uvarint(data, &mut pos)? as usize;
        if entry_len > MAX_BLOCK_SIZE {
            return Err(CarError::BlockTooLarge(MAX_BLOCK_SIZE));
        }
        let entry_end = pos.checked_add(entry_len).ok_or(CarError::Truncated)?;
        let entry_bytes = data.get(pos..entry_end).ok_or(CarError::Truncated)?;
        pos = entry_end;

        let mut cursor = Cursor::new(entry_bytes);
        let cid =
            Cid::read_bytes(&mut cursor).map_err(|e| CarError::Cid(e.to_string()))?;
        let cid_len = cursor.position() as usize;
        let block_bytes = entry_bytes
            .get(cid_len..)
            .ok_or(CarError::Truncated)?
            .to_vec();
        blocks.push((cid, block_bytes));
    }

    Ok(CarFile { roots, blocks })
}

/// ブロック探索用に `Vec<(Cid, Vec<u8>)>` を `HashMap` へ変換するヘルパー。
pub fn blocks_to_map(blocks: Vec<(Cid, Vec<u8>)>) -> HashMap<Cid, Vec<u8>> {
    blocks.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atp::repo::{build_mst, cid_from_dagcbor, encode_car};

    #[test]
    fn round_trips_empty_car() {
        let (root, blocks) = build_mst(&[]).unwrap();
        let car = encode_car(&root, &blocks).unwrap();
        let decoded = decode_car(&car).unwrap();
        assert_eq!(decoded.roots, vec![root]);
        assert_eq!(decoded.blocks.len(), blocks.len());
    }

    #[test]
    fn round_trips_populated_car() {
        let entries: Vec<(String, Cid)> = (0..20)
            .map(|i| {
                let cbor = format!("record-{i}").into_bytes();
                (format!("app.bsky.feed.post/key{i:03}"), cid_from_dagcbor(&cbor))
            })
            .collect();
        let (root, blocks) = build_mst(&entries).unwrap();
        let car = encode_car(&root, &blocks).unwrap();

        let decoded = decode_car(&car).unwrap();
        assert_eq!(decoded.roots, vec![root]);
        assert_eq!(decoded.blocks.len(), blocks.len());

        let decoded_map = blocks_to_map(decoded.blocks);
        for (cid, bytes) in &blocks {
            assert_eq!(decoded_map.get(cid), Some(bytes));
        }
    }

    #[test]
    fn rejects_truncated_input() {
        let (root, blocks) = build_mst(&[("a/b".to_string(), cid_from_dagcbor(b"x"))]).unwrap();
        let car = encode_car(&root, &blocks).unwrap();
        let truncated = &car[..car.len() - 3];
        assert!(decode_car(truncated).is_err());
    }

    #[test]
    fn rejects_empty_input() {
        assert!(matches!(decode_car(&[]), Err(CarError::Empty)));
    }
}
