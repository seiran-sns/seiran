# SKILL: AT Protocol Rust プログラミング実装ガイド

このドキュメントは seiran 開発中に得た AT Protocol の Rust 実装知識をまとめたものです。
デバッグで苦労した落とし穴を中心に記録します。

---

## 1. did:plc 登録（plc.directory）

### 1-1. 全体フロー

```
① ユーザー固有の P-256 署名鍵ペアを生成
② サーバーの P-256 ローテーション鍵ペアを secrets.toml からロード
③ genesis operation（未署名）を構築
④ 未署名 operation を DAG-CBOR エンコード → ローテーション鍵で署名
⑤ 署名を付けた signed operation を DAG-CBOR エンコード → SHA-256 → base32 → DID
⑥ POST https://plc.directory/{DID}  body = signed operation (JSON)
```

### 1-2. 鍵の役割

| 鍵 | 目的 | 保管場所 |
|---|---|---|
| **ローテーション鍵（サーバー共通）** | genesis operation への署名、DID の制御権 | `secrets.toml` の `atproto_private_key_pem` |
| **署名鍵（ユーザー固有）** | MST コミットへの署名 | `actors.at_signing_key_pem` |

### 1-3. genesis operation の構造

```json
{
  "alsoKnownAs": ["at://username.your-domain.example"],
  "prev": null,
  "rotationKeys": ["did:key:z..."],
  "services": {
    "atproto_pds": {
      "endpoint": "https://your-domain.example",
      "type": "AtprotoPersonalDataServer"
    }
  },
  "type": "plc_operation",
  "verificationMethods": {
    "atproto": "did:key:z..."
  }
}
```

---

## 2. 落とし穴集（デバッグ済み）

### 落とし穴①: 署名の base64 エンコーディング

**❌ 間違い:** `base64::engine::general_purpose::STANDARD`（base64pad、`=` パディングあり）

**✅ 正解:** `base64::engine::general_purpose::URL_SAFE_NO_PAD`（base64url、パディングなし）

**理由:** plc.directory のソースコード（`@did-plc/lib`）が sig に `=` が含まれると即座に `InvalidSignatureError` を throw する。

```typescript
// plc.directory 検証コード（TypeScript）
if (sig.endsWith('=')) {
    throw new InvalidSignatureError(op)  // ← 絶対に '=' を付けてはいけない
}
const sigBytes = uint8arrays.fromString(sig, 'base64url')
```

```rust
// ✅ Rust での正解
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
let sig_str = URL_SAFE_NO_PAD.encode(raw_sig.to_bytes().as_slice());
```

---

### 落とし穴②: DID の計算対象

**❌ 間違い:** 未署名 operation の DAG-CBOR ハッシュから DID を計算する

**✅ 正解:** **署名済み** operation（`sig` フィールド含む）の DAG-CBOR ハッシュから DID を計算する

```rust
// ✅ 正しい順序
// Step 1: 未署名 op を DAG-CBOR エンコード → 署名
let unsigned_cbor = serde_ipld_dagcbor::to_vec(&unsigned_op)?;
let sig: p256::ecdsa::Signature = rotation_signing_key.sign(&unsigned_cbor);
let sig_str = URL_SAFE_NO_PAD.encode(sig.to_bytes().as_slice());

// Step 2: 署名済み op を DAG-CBOR エンコード → SHA-256 → DID
let signed_op = SignedGenesisOp { ..., sig: sig_str };
let signed_cbor = serde_ipld_dagcbor::to_vec(&signed_op)?;
let hash = Sha256::digest(&signed_cbor);
let b32 = base32::encode(base32::Alphabet::RFC4648 { padding: false }, hash.as_slice())
    .to_lowercase();
let did = format!("did:plc:{}", &b32[..24]);
```

**plc.directory がやっていること:**
```typescript
// plc.directory 検証コード
const expectedDid = await didForCreateOp(parsed)  // parsed = signed op including sig
if (expectedDid !== did) {
    return { ok: false, message: "Hash of genesis operation does not match..." }
}
```

---

### 落とし穴③: P-256 の p256 クレート署名メソッド

**✅ `Signer::sign` は内部で SHA-256 を自動適用する（事前ハッシュ不要）**

```rust
use p256::ecdsa::{signature::Signer, SigningKey};

// ✅ これで正しい。内部で SHA-256(cbor_bytes) → ECDSA-P256 を実行する
let sig: p256::ecdsa::Signature = signing_key.sign(&cbor_bytes);
```

AT Protocol の TypeScript 実装（`@atproto/crypto`）も同様：
```typescript
// @atproto/crypto P256Keypair.sign
const msgHash = await sha256(msg)          // SHA-256 を明示的に計算
const sig = p256.sign(msgHash, this.privateKey)  // noble/curves へは pre-hash を渡す
```
Rust の `sign()` は内部でこれと等価な処理を行う。

**署名の出力形式:** IEEE P1363（R || S、64 バイト）
```rust
let sig_bytes: [u8; 64] = sig.to_bytes().into();  // R(32bytes) + S(32bytes)
```

---

### 落とし穴④: DAG-CBOR のキーソート

`serde_ipld_dagcbor` はすべての map/struct のキーを自動的に **canonical 順**（バイト長 ASC → 辞書順 ASC）でソートする。
Rust の struct のフィールド宣言順は関係ない。

**canonical ソート例（genesis op のキー）:**

| キー | バイト長 | canonical 順位 |
|---|---|---|
| `prev` | 4 | 1位 |
| `type` | 4 | 2位（`p` < `t`） |
| `services` | 8 | 3位 |
| `alsoKnownAs` | 11 | 4位 |
| `rotationKeys` | 12 | 5位 |
| `verificationMethods` | 19 | 6位 |

signed op に `sig`（3バイト）が加わると先頭になる：`sig`, `prev`, `type`, ...

---

## 3. did:key エンコーディング（P-256）

### 3-1. フォーマット

```
did:key:z + base58btc([0x80, 0x24] + compressed_p256_pubkey)
```

- `[0x80, 0x24]` = multicodec `p256-pub` (0x1200) の varint エンコーディング
- `compressed_p256_pubkey` = 33 バイト（02 or 03 プレフィックス）
- 合計 35 バイトを base58btc エンコードして `z` プレフィックスを付ける

```rust
pub fn p256_to_did_key(verifying_key: &p256::ecdsa::VerifyingKey) -> String {
    let compressed = verifying_key.to_encoded_point(true); // 33 bytes
    let mut buf = vec![0x80u8, 0x24u8];                    // varint(0x1200)
    buf.extend_from_slice(compressed.as_bytes());
    format!("did:key:z{}", bs58::encode(&buf).into_string())
}
```

P-256 の did:key は `zDnaew...` で始まる文字列になる（secp256k1 の `zQ3sh...` とは異なる）。

---

## 4. 必要な Rust クレート

```toml
[dependencies]
p256 = { version = "0.13", features = ["pkcs8", "pem"] }
serde_ipld_dagcbor = "0.6"
bs58 = "0.5"
base32 = "0.4"
base64 = "0.22"
sha2 = "0.10"
```

### バージョン別の注意点

- `base32 = "0.4"`: アルファベット variant は `Alphabet::RFC4648 { padding: false }` を使い、`.to_lowercase()` で小文字化する（`Rfc4648Lower` は 0.5+ から）
- `serde_ipld_dagcbor = "0.6"`: struct も BTreeMap も自動で canonical ソートされる

---

## 5. plc.directory API

| 項目 | 内容 |
|---|---|
| エンドポイント | `POST https://plc.directory/{did}` |
| Content-Type | `application/json` |
| ボディ | 署名済み operation（JSON） |
| 成功レスポンス | HTTP 200 or 201（ボディなし or `{}`） |
| エラー例 | `{"message": "Hash of genesis operation does not match DID identifier: ..."}` |
| エラー例 | `{"message": "Invalid signature on op: ..."}` |

---

## 6. DID 解決（did:plc / did:web）

```rust
// did:plc → https://plc.directory/{did}
// did:web:example.com → https://example.com/.well-known/did.json
// did:web:example.com:path:to → https://example.com/path/to/did.json
```

DID Document の重要フィールド:
- `alsoKnownAs`: AT ハンドル（`at://username.domain`）
- `service[].type == "AtprotoPersonalDataServer"` の `endpoint`: PDS の URL
- `verificationMethod[].publicKeyMultibase`: 署名検証用公開鍵

---

## 7. AT Protocol のハンドル検証

登録したハンドル（`at://username.domain`）をクライアントが検証する際、以下のいずれかが必要：

**方法A: DNS TXT レコード**
```
_atproto.username.domain.  IN TXT  "did=did:plc:xxxx"
```

**方法B: HTTPS Well-Known**
```
GET https://username.domain/.well-known/atproto-did
→ レスポンス: did:plc:xxxx
```

seiran では `*.beta.seiran.org` のワイルドカード DNS + HTTPS エンドポイントで対応予定。

---

## 8. AT Protocol とサブエージェント調査

複雑な仕様の調査には `Explore` サブエージェントが有効。以下の公式リポジトリを参照させる：

- `https://github.com/did-method-plc/did-method-plc` — plc.directory 参照実装
- `https://github.com/bluesky-social/atproto/tree/main/packages/crypto` — `@atproto/crypto`
- `https://github.com/sugyan/atrium` — Rust AT Protocol SDK（公式ではないが包括的）
- `@noble/curves` の p256 — AT Protocol が採用している P-256 実装
