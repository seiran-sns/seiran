//! MST (Merkle Search Tree) ウォーク — CARから取り出した全ブロックを元に、
//! ルートCIDから全レコード（リーフ）を列挙する。`atp/repo.rs::build_mst` の逆方向。
//!
//! ノード形式（`repo.rs::MstNode`/`MstEntry` 参照、canonical CBORキー順 e < l, k < p < t < v）:
//!   node = { e: [entry, ...], l: Option<Cid> }
//!   entry = { k: bytes（前エントリとの共通prefixを除いた差分キー）, p: u32（共通prefix長）,
//!              t: Option<Cid>（右側部分木）, v: Cid（レコード自体のCID） }
//!
//! 外部（信頼できないPDS）由来のデータを辿るため、再帰深さの上限を必須とする。

use super::car::{blocks_to_map, decode_car, CarError, MAX_MST_DEPTH};
use ipld_core::cid::Cid;
use ipld_core::ipld::Ipld;
use std::collections::HashMap;

/// MSTから列挙された1レコード。
pub struct MstLeaf {
    /// フルキー（例: "app.bsky.feed.post/3jxyz..."）。
    pub key: String,
    pub cid: Cid,
    /// レコード本体の生DAG-CBORバイト列（無加工）。
    pub bytes: Vec<u8>,
}

fn ipld_map<'a>(ipld: &'a Ipld, what: &str) -> Result<&'a std::collections::BTreeMap<String, Ipld>, CarError> {
    match ipld {
        Ipld::Map(m) => Ok(m),
        _ => Err(CarError::Node(format!("{what}がMap型ではありません"))),
    }
}

fn optional_link(map: &std::collections::BTreeMap<String, Ipld>, key: &str) -> Result<Option<Cid>, CarError> {
    match map.get(key) {
        None | Some(Ipld::Null) => Ok(None),
        Some(Ipld::Link(cid)) => Ok(Some(*cid)),
        _ => Err(CarError::Node(format!(
            "{key}フィールドがCIDリンクでもnullでもありません"
        ))),
    }
}

/// ルートCIDから全リーフを深さ優先（キー順）で列挙する。
pub fn walk_mst(root: Cid, blocks: &HashMap<Cid, Vec<u8>>) -> Result<Vec<MstLeaf>, CarError> {
    let mut leaves = Vec::new();
    walk_node(root, blocks, &mut leaves, 0)?;
    Ok(leaves)
}

fn walk_node(
    cid: Cid,
    blocks: &HashMap<Cid, Vec<u8>>,
    leaves: &mut Vec<MstLeaf>,
    depth: usize,
) -> Result<(), CarError> {
    if depth > MAX_MST_DEPTH {
        return Err(CarError::TooDeep(MAX_MST_DEPTH));
    }
    let bytes = blocks
        .get(&cid)
        .ok_or_else(|| CarError::MissingBlock(cid.to_string()))?;
    let node: Ipld =
        serde_ipld_dagcbor::from_slice(bytes).map_err(|e| CarError::Node(e.to_string()))?;
    let map = ipld_map(&node, "MSTノード")?;

    // 左端のサブツリー（layer最左、全キーより小さい）を先に辿る。
    if let Some(left) = optional_link(map, "l")? {
        walk_node(left, blocks, leaves, depth + 1)?;
    }

    let Some(Ipld::List(entries)) = map.get("e") else {
        return Err(CarError::Node("eフィールドがありません".into()));
    };

    let mut prev_key: Vec<u8> = Vec::new();
    for entry in entries {
        let entry_map = ipld_map(entry, "MSTエントリ")?;

        let p = match entry_map.get("p") {
            Some(Ipld::Integer(n)) if *n >= 0 => *n as usize,
            _ => return Err(CarError::Node("pフィールドが不正です".into())),
        };
        let k_suffix = match entry_map.get("k") {
            Some(Ipld::Bytes(b)) => b.clone(),
            _ => return Err(CarError::Node("kフィールドが不正です".into())),
        };
        if p > prev_key.len() {
            return Err(CarError::Node(
                "pが直前キーの長さを超えています（不正なMST）".into(),
            ));
        }

        let mut full_key = prev_key[..p].to_vec();
        full_key.extend_from_slice(&k_suffix);
        prev_key = full_key.clone();

        let v_cid = match entry_map.get("v") {
            Some(Ipld::Link(c)) => *c,
            _ => return Err(CarError::Node("vフィールドが不正です".into())),
        };
        let key = String::from_utf8(full_key).map_err(|_| CarError::InvalidKey)?;
        let record_bytes = blocks
            .get(&v_cid)
            .ok_or_else(|| CarError::MissingBlock(v_cid.to_string()))?
            .clone();
        leaves.push(MstLeaf {
            key,
            cid: v_cid,
            bytes: record_bytes,
        });

        // このエントリの右側（次エントリより小さい範囲）のサブツリー。
        if let Some(right) = optional_link(entry_map, "t")? {
            walk_node(right, blocks, leaves, depth + 1)?;
        }
    }

    Ok(())
}

/// リーフの `key`（"collection/rkey"）を分割する。
pub fn split_record_key(key: &str) -> Result<(String, String), CarError> {
    key.split_once('/')
        .map(|(collection, rkey)| (collection.to_string(), rkey.to_string()))
        .ok_or_else(|| CarError::MalformedKey(key.to_string()))
}

/// 1レコードの取り込み結果。`collection`/`rkey`ごとに投入先テーブルを分岐させるための
/// 最小限の情報（`app.bsky.feed.post`は`posts`テーブルへ、それ以外は`atp_records`へ、という
/// 分岐は呼び出し側＝転入インポートジョブの責務。ここでは生バイト列のまま返すのみ）。
pub struct DecodedRecord {
    pub collection: String,
    pub rkey: String,
    pub cid: Cid,
    pub bytes: Vec<u8>,
}

/// `com.atproto.sync.getRepo` が返すCARファイル全体を受け取り、
/// 全レコード（`posts`/`atp_records`へ投入する生バイト列）を列挙する。
///
/// CARのroot（commitブロック）から `data`（MSTルートCID）を取り出し、そこから
/// `walk_mst` で全リーフを辿る。ネットワークI/Oを含まない純粋関数。
pub fn decode_repo_records(car_bytes: &[u8]) -> Result<Vec<DecodedRecord>, CarError> {
    let car = decode_car(car_bytes)?;
    let root = *car.roots.first().ok_or(CarError::Empty)?;
    let blocks = blocks_to_map(car.blocks);

    let commit_bytes = blocks
        .get(&root)
        .ok_or_else(|| CarError::MissingBlock(root.to_string()))?;
    let commit: Ipld =
        serde_ipld_dagcbor::from_slice(commit_bytes).map_err(|e| CarError::Node(e.to_string()))?;
    let commit_map = ipld_map(&commit, "commit")?;
    let mst_root = match commit_map.get("data") {
        Some(Ipld::Link(cid)) => *cid,
        _ => return Err(CarError::Node("commitにdataフィールドがありません".into())),
    };

    let leaves = walk_mst(mst_root, &blocks)?;
    leaves
        .into_iter()
        .map(|leaf| {
            let (collection, rkey) = split_record_key(&leaf.key)?;
            Ok(DecodedRecord {
                collection,
                rkey,
                cid: leaf.cid,
                bytes: leaf.bytes,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atp::repo::build_mst;

    fn record_bytes(i: usize) -> Vec<u8> {
        format!("record-{i}").into_bytes()
    }

    fn record_cid(i: usize) -> Cid {
        crate::atp::repo::cid_from_dagcbor(&record_bytes(i))
    }

    #[test]
    fn walks_all_leaves_in_key_order() {
        let mut entries: Vec<(String, Cid)> = (0..37)
            .map(|i| (format!("app.bsky.feed.post/key{i:04}"), record_cid(i)))
            .collect();
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));

        let (root, mst_blocks) = build_mst(&entries).unwrap();
        // MSTノード自体のブロックに加え、各リーフが指すレコード本体のブロックも
        // block map に入れておく必要がある（実際のCARには両方含まれる）。
        let mut block_map: HashMap<Cid, Vec<u8>> = mst_blocks.into_iter().collect();
        for i in 0..37 {
            block_map.insert(record_cid(i), record_bytes(i));
        }

        let leaves = walk_mst(root, &block_map).unwrap();
        let leaf_keys: Vec<String> = leaves.iter().map(|l| l.key.clone()).collect();
        let expected_keys: Vec<String> = entries.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(leaf_keys, expected_keys);

        for (leaf, (_, expected_cid)) in leaves.iter().zip(entries.iter()) {
            assert_eq!(leaf.cid, *expected_cid);
            assert_eq!(leaf.bytes, block_map.get(expected_cid).unwrap().clone());
        }
    }

    #[test]
    fn walks_empty_tree() {
        let (root, blocks) = build_mst(&[]).unwrap();
        let block_map: HashMap<Cid, Vec<u8>> = blocks.into_iter().collect();
        let leaves = walk_mst(root, &block_map).unwrap();
        assert!(leaves.is_empty());
    }

    #[test]
    fn split_record_key_works() {
        assert_eq!(
            split_record_key("app.bsky.feed.post/abc123").unwrap(),
            ("app.bsky.feed.post".to_string(), "abc123".to_string())
        );
        assert!(split_record_key("no-slash-here").is_err());
    }

    #[test]
    fn decode_repo_records_round_trips_full_car() {
        use crate::atp::repo::{create_commit, encode_car};
        use p256::ecdsa::SigningKey;

        let mut entries: Vec<(String, Cid)> = (0..12)
            .map(|i| (format!("app.bsky.feed.post/rk{i:03}"), record_cid(i)))
            .chain((0..5).map(|i| (format!("app.bsky.actor.profile/rk{i:03}"), record_cid(100 + i))))
            .collect();
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));

        let (mst_root, mst_blocks) = build_mst(&entries).unwrap();
        let signing_key = SigningKey::random(&mut rand_core_shim());
        let (commit_cid, commit_cbor) =
            create_commit("did:plc:test1234", "3jxyz000000", mst_root, None, &signing_key).unwrap();

        let mut all_blocks = mst_blocks;
        all_blocks.push((commit_cid, commit_cbor));
        // レコード本体のブロックも実CAR同様に含める。
        for i in 0..12 {
            all_blocks.push((record_cid(i), record_bytes(i)));
        }
        for i in 0..5 {
            all_blocks.push((record_cid(100 + i), record_bytes(100 + i)));
        }
        let car = encode_car(&commit_cid, &all_blocks).unwrap();

        let records = decode_repo_records(&car).unwrap();
        assert_eq!(records.len(), entries.len());

        let mut got: Vec<(String, String, Cid)> = records
            .iter()
            .map(|r| (r.collection.clone(), r.rkey.clone(), r.cid))
            .collect();
        got.sort();
        let mut want: Vec<(String, String, Cid)> = entries
            .iter()
            .map(|(k, cid)| {
                let (c, rk) = split_record_key(k).unwrap();
                (c, rk, *cid)
            })
            .collect();
        want.sort();
        assert_eq!(got, want);
    }

    // p256::ecdsa::SigningKey::random は rand_core 0.6 系の RngCore を要求する。
    // argon2 が再エクスポートする OsRng を使い、secrets.rs と同じ回避策を踏襲する。
    fn rand_core_shim() -> argon2::password_hash::rand_core::OsRng {
        argon2::password_hash::rand_core::OsRng
    }

    /// `@testimport.bsky.social`（did:plc:n3tpur22sxb57zgzwe3lkl5m、実PDS
    /// `brittlegill.us-west.host.bsky.network`）から2026-09-08に実際に取得した
    /// `com.atproto.sync.getRepo`のレスポンス。転入フロー検証専用のテストアカウントで、
    /// 自由に使ってよいことを確認済み。実PDS実装との相互運用性を確認するための固定フィクスチャ。
    #[test]
    fn decodes_real_bsky_social_repo_fixture() {
        let car_bytes = include_bytes!("testdata/testimport_repo.car");
        let records = decode_repo_records(car_bytes).unwrap();
        // 新規テストアカウントのため投稿等は無い可能性が高いが、
        // 「エラーなく最後まで読み切れる」こと自体が実データとの相互運用性の検証になる。
        for record in &records {
            assert!(!record.collection.is_empty());
            assert!(!record.rkey.is_empty());
        }
    }

    #[test]
    fn rejects_missing_block() {
        let bogus_root = record_cid(999);
        let empty_blocks: HashMap<Cid, Vec<u8>> = HashMap::new();
        assert!(matches!(
            walk_mst(bogus_root, &empty_blocks),
            Err(CarError::MissingBlock(_))
        ));
    }
}
