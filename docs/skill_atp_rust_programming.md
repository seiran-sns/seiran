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
ipld-core = "0.4"     # Ipld 型（CIDリンクを含むCBOR ⇄ JSON の相互変換に必須。§14参照）
jsonwebtoken = "9"    # Service Auth JWT の署名（§11・§12参照。low-S正規化は自前で行う必要あり）
infer = "0.16"        # マジックバイトからのMIMEタイプ判定（§13の Content-Type: */* 対策）
```

### バージョン別の注意点

- `base32 = "0.4"`: アルファベット variant は `Alphabet::RFC4648 { padding: false }` を使い、`.to_lowercase()` で小文字化する（`Rfc4648Lower` は 0.5+ から）
- `serde_ipld_dagcbor = "0.6"`: struct も BTreeMap も自動で canonical ソートされる
- `jsonwebtoken = "9"`（内部 `ring` バックエンド）: ES256 署名を low-S に正規化するオプションが無い。§11 の後処理が必須

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

---

## 9. indigo/relay をローカルセルフホストして実地検証する

Bsky への配送が「一部だけ・特定タイミングだけ失敗する」といった再現しにくい不具合は、
本番の `bsky.network` を相手にしていては検証ログが得られない。
`bluesky-social/indigo`（公式 relay/BGS 実装, Go）をローカルで動かし、
自分の PDS から実際にコミットを流し込んで検証ログを直接読むのが最速。

### 9-1. 手順

```bash
# Go は mise で導入する（バージョン管理ツールを使わず ~/.local 等に手動インストールすると
# 環境の再現性が下がるため非推奨）
mise use -g go@latest

git clone https://github.com/bluesky-social/indigo
cd indigo && go build -o relay ./cmd/relay

./relay serve --db-url sqlite://relay.sqlite --admin-password xxx --log-level debug
# デフォルトで :2470 で待受
```

### 9-2. seiran 側の対応

`crates/seiran-common/src/atp/service.rs` の `spawn_request_crawl()` と
`crates/seiran-api/src/lib.rs` の `request_relay_crawl()` は
`ATP_RELAY_URL` 環境変数（カンマ区切りで複数指定可）で通知先を制御できる。

```bash
# .env: 本番配送を維持したままローカルrelayにも並行通知する
ATP_RELAY_URL=https://bsky.network,http://localhost:2470
```

テストユーザー（`seiran1` 等）で投稿・リポスト等を行い、`./relay` の標準出力を
`tail -f` して `"commit message failed verification"` のような WARN/ERROR を探す。
本番 relay は失敗理由を返さず単に配送が欠落するだけだが、ローカル relay の検証コード
（`cmd/relay/relay/verify.go` の `VerifyCommitMessageStrict` 等）はエラーメッセージ付きで
リジェクトするため、原因の当たりが劇的に付けやすくなる。

**Why:** 「配送が全体的に不安定でその原因はわかっていない」という抽象的な報告から出発し、
この手法で [[project_seiran_bsky_relay_delivery_instability]] の2大原因（後述）を特定できた。

---

## 10. AT Protocol Sync 1.1: `#commit` フレームの `prevData` 必須化

`subscribeRepos` で流す `#commit` イベントには、2回目以降のコミットで
**`prevData`（前回コミット時点の MST root CID）が必須**になっている
（indigo `VerifyCommitMessageStrict` で検証）。初回コミットのみ省略可。

```rust
// crates/seiran-common/src/atp/repo.rs
pub fn build_commit_frame(
    // ...
    prev_data: Option<&Cid>,  // 前回コミットの MST root CID
) -> Result<Vec<u8>, RepoError> {
    // ...
    if let Some(c) = prev_data {
        body_map.insert("prevData".to_string(), Ipld::Link(*c));
    }
    // ...
}
```

これを保持するには、コミットのたびに MST root CID を DB に永続化しておく必要がある
（seiran では `actors.at_repo_data_cid` カラムで対応）。欠落させると
`"missing prevData field"` で即リジェクトされる。**症状は「特定タイミングで配送が途絶える」
ように見えるが、実際には「2回目以降のコミットが常に失敗する」という一貫した不具合**であり、
タイミング依存ではないことに注意（1回目のコミットだけはたまたま通るため不規則に見える）。

---

## 11. ECDSA 署名マレアビリティ対策: "low-S" 正規化【最重要の落とし穴】

AT Protocol の署名検証（indigo `atcrypto.PublicKeyP256.HashAndVerify` 等）は
ECDSA のマレアビリティ対策として **low-S 形式の署名を必須とする**。
RustCrypto 系クレート（`p256::ecdsa`）・`jsonwebtoken`（内部 `ring`）は
どちらも **デフォルトでは low-S に正規化しない**。ECDSA 署名の s 値は数学的性質上
ほぼ50%の確率で high-S になるため、正規化を怠ると **署名の約半分がランダムに
`"cryptographic signature invalid"` でリジェクトされる**（実測: MST commit 200件中106件、
Service Auth JWT 300件中152件が high-S だった）。

「配送が不安定」「一部だけ失敗する」という報告の多くは、実はタイミングや負荷と無関係な
コイントスの確率で起きていることがある。まずこれを疑うとよい。

**normalize_s() が必要な箇所は署名を行うすべての場所——MST commit だけでなく
PLC genesis operation、Service Auth JWT も含む。1箇所直して終わりにしない。**

```rust
// MST commit 署名（crates/seiran-common/src/atp/repo.rs）
let sig: p256::ecdsa::Signature = signing_key.sign(&unsigned_cbor);
let sig = sig.normalize_s().unwrap_or(sig);

// PLC genesis operation 署名（crates/seiran-common/src/atp/plc.rs）も同様
```

`jsonwebtoken`（`ring` バックエンド）は署名生成時に normalize_s() を挟む API が無いため、
**JWT を作った後で署名セグメントだけをデコードし直して矯正する**しかない:

```rust
// crates/seiran-common/src/atp/service_auth.rs
fn normalize_jwt_es256_signature(jwt: &str) -> Result<String, ServiceAuthError> {
    let mut parts = jwt.rsplitn(2, '.');
    let sig_b64 = parts.next()?;
    let header_payload = parts.next()?;

    let sig_bytes = URL_SAFE_NO_PAD.decode(sig_b64)?;
    let sig = Signature::try_from(sig_bytes.as_slice())?;
    let normalized = sig.normalize_s().unwrap_or(sig);
    let normalized_b64 = URL_SAFE_NO_PAD.encode(normalized.to_bytes());

    Ok(format!("{}.{}", header_payload, normalized_b64))
}
```

---

## 12. Service Auth JWT（`com.atproto.repo.uploadBlob` 等への自己署名認証）

`app.bsky.video.uploadVideo` のように、外部サービスが自分の代わりに自分の PDS を
呼び返してくる（＝アカウントの署名鍵で自己署名した JWT を持たせて代理アクセスさせる）
仕組みが AT Protocol にはある。クレームは `iss`（呼び出し元 DID）・`aud`（呼び出し先
サービス DID、`#fragment` 込み可）・`lxm`（呼び出す XRPC メソッド名）・`exp`（60秒推奨）。

```rust
pub fn sign_service_auth_jwt(pem: &str, iss: &str, aud: &str, lxm: &str) -> Result<String, ServiceAuthError> {
    let claims = ServiceAuthClaims { iss, aud, lxm, exp: Utc::now().timestamp() + 60 };
    let key = EncodingKey::from_ec_pem(pem.as_bytes())?;
    let jwt = encode(&Header::new(Algorithm::ES256), &claims, &key)?;
    normalize_jwt_es256_signature(&jwt)  // §11 参照、忘れると約50%失敗する
}
```

**`lxm` は「JWT を使って最終的に叩かれるエンドポイント名」であり、JWT を発行した
エンドポイント名ではない**ことに注意。`app.bsky.video.uploadVideo` を叩くために
発行した JWT でも、動画サービスがトランスコード後に折り返して
`com.atproto.repo.uploadBlob` を呼ぶ際は `lxm = "com.atproto.repo.uploadBlob"` の
別 JWT を使う（≒公式 `social-app` クライアントの実装と同じ設計）。ここを取り違えて
「lxm ミスマッチ」を疑ったが実際は無関係だった、という誤診断の経験あり。

---

## 13. `uploadBlob` は受信バイト列を必ず保存する（読み捨てると外部サービスから 404 になる）

自 PDS 上の表示・Fedi配信では、添付ファイルは元々アップロード済みのオリジナルファイルを
使うため、`com.atproto.repo.uploadBlob` で受け取ったバイト列自体は「不要に見える」。
しかし **`video.bsky.app` はトランスコード完了後、このエンドポイントへ自分から
代理 POST してきてトランスコード済みバイナリを渡してくる**（§12 の `lxm` 切り替えは
このため）。そして `video.bsky.app` 自身が後で動画再生のために **同じ CID を
`com.atproto.sync.getBlob` で取得しにくる**。ここで読み捨てていると常に 404 になり、
Bsky公式アプリ上で「ビデオが見つかりません」となって再生不能になる。

対策として、`uploadBlob` で受けたバイト列は必ずどこかに保存し、`getBlob` で
引けるようにする。seiran では `media_files`（ユーザー添付ファイル）とは別に
`atp_blobs`（サーバー間中間生成物）テーブルを新設して保存している。

**分離テーブルにする場合の安全対策（3点、無制限アップロード窓口になりがちなので必須）:**
1. pending な動画ジョブに紐付いているリクエストのみ受理する（無関係な無制限アップロードのDoS経路にしない）
2. `media_files` 側と sha256 でクロステーブル重複排除する（同一内容をS3に二重保存しない）
3. 一定期間（seiranでは7日）で使われなくなった blob を GC する

また、`video.bsky.app` からの代理 POST は実機で **`Content-Type: */*`** という
無効なワイルドカード値を送ってくることがある。そのまま保存すると配信時の
Content-Type も `*/*` になり再生できないため、ヘッダーに `*` を含む場合は
マジックバイトから実際の MIME type を判定（sniff）してから保存する。

---

## 14. `getRecord` で CID リンクを含む CBOR を JSON に変換する（`Ipld` 経由が必須）

DAG-CBOR で保存したレコード（embed に画像/動画/引用等の CID リンクを含む）を
`com.atproto.repo.getRecord` のレスポンス JSON に変換する際、CBOR バイト列を
**`serde_json::Value` へ直接デシリアライズすることはできない**。CID リンク
（DAG-CBOR tag 42）が `serde_json::Value` にとって未知の型になり、
`invalid type: newtype struct` のようなエラーで静かに失敗する
（フォールバック実装があると気付かず放置されがちなので注意）。

正しくは一度 `ipld_core::ipld::Ipld` にデシリアライズしてから、
AT Protocol 標準の JSON 表現（CIDリンクは `{"$link": "..."}`、バイト列は
`{"$bytes": "<base64url>"}`）へ手動変換する。

```rust
fn ipld_to_json(ipld: &ipld_core::ipld::Ipld) -> serde_json::Value {
    use ipld_core::ipld::Ipld;
    match ipld {
        Ipld::Null => serde_json::Value::Null,
        Ipld::Bool(b) => serde_json::Value::Bool(*b),
        Ipld::Integer(i) => serde_json::json!(i),
        Ipld::Float(f) => serde_json::json!(f),
        Ipld::String(s) => serde_json::Value::String(s.clone()),
        Ipld::Bytes(b) => serde_json::json!({ "$bytes": URL_SAFE_NO_PAD.encode(b) }),
        Ipld::List(l) => serde_json::Value::Array(l.iter().map(ipld_to_json).collect()),
        Ipld::Map(m) => serde_json::Value::Object(m.iter().map(|(k, v)| (k.clone(), ipld_to_json(v))).collect()),
        Ipld::Link(cid) => serde_json::json!({ "$link": cid.to_string() }),
    }
}
```

`posts.body` 等の別テーブルから JSON をその場で再構築する簡易実装は、
embed 情報（画像・動画・引用等）を一切含められないので要注意。
`atp_blocks` に保存した実際の DAG-CBOR ブロックを読み直し、この変換を通すのが正攻法。

---

## 15. Bsky 公式動画パイプライン（`app.bsky.embed.video`）の実測制約

`app.bsky.video.uploadVideo` → `getJobStatus` のポーリングで結合する際、
実機検証で分かった制約（2026-07-17時点、Bsky側の非公開仕様なので今後変わりうる）:

| 項目 | 結果 |
|---|---|
| アスペクト比 6.87:1（515x75） | `uploadVideo` が `"video aspect ratio is too wide"` で即座に拒否 |
| アスペクト比 2:1（600x300）・3:1（600x200/180x60） | 許容される |
| 解像度 36x12（3:1、432px²） | `uploadVideo` は通るが `getJobStatus` が `"video processing error"` で失敗（小さすぎる） |
| 解像度 80x20（4:1、1600px²） | 実機で再生確認済み |

上限アスペクト比は「6.87:1 と 3:1 未満のどこか」、解像度下限は
「432px² と 1600px² の間のどこか」で、いずれも未確定（二分探索で詰められる）。

**`getJobStatus` のエラー詳細を握りつぶすバグに注意:** `jobId` フィールドが
空文字列 `""` で返ってくることがあり、`Option` 型に素朴に詰めると
`Some("")` として「有効な jobId が来た」扱いになってしまい、実際は
`{"error": "video aspect ratio is too wide"}` のようなエラー詳細が
ログに出ないままポーリングが延々ハングする。

```rust
// crates/seiran-api/src/handlers/drive.rs
let job_id = extracted.filter(|s: &String| !s.is_empty());
```

**`app.bsky.embed.video` の `size` フィールドはオリジナルファイルサイズではない。**
Bsky側でトランスコードされたバイト列サイズを使う必要があり、両者は大きく異なる
（実測: 2,867,780 → 287,123 バイトのように変わる）。`getJobStatus` 完了時の
`blob.size` を DB に保存して使う（seiran では `media_files.bsky_video_size`）。

---

## 16. 音声投稿を動画パイプラインに載せて配信するワークアラウンド

AT Protocol の `app.bsky.embed.*` には**音声専用の embed type が存在しない**。
音声を「グレー背景の静止画＋音声トラック」の mp4 動画に `ffmpeg` で変換し、
既存の動画パイプライン（§15）にそのまま載せることで `app.bsky.embed.video`
として配信できる（seiran発案の実用ワークアラウンド）。

```rust
// crates/seiran-common/src/storage/media_probe.rs
let color_src = format!("color=c=gray:s={}x{}:r=2", AUDIO_VIDEO_WIDTH, AUDIO_VIDEO_HEIGHT); // 80x20, 2fps
let mut child = Command::new("ffmpeg")
    .args(["-y", "-f", "lavfi", "-i", &color_src])
    .arg("-i").arg(audio_path)
    .args([
        "-shortest",
        "-c:v", "libx264", "-pix_fmt", "yuv420p",
        "-c:a", "aac", "-b:a", "128k",
        "-movflags", "frag_keyframe+empty_moov",  // pipe:1（シーク不可）でも mp4 を書き出すのに必須
        "-f", "mp4", "pipe:1",
    ])
    .stdout(std::process::Stdio::piped())
    ...
```

低フレームレート（2fps）でファイルサイズを抑制（5秒の音声で変換後約90KB）。
表示時に引き延ばせるので解像度は小さくてよく、§15 の下限に近い 80x20（4:1）を採用。
`ffmpeg` 未インストール・変換失敗時は `None` を返し、呼び出し側は従来通り
`app.bsky.embed.external`（簡易視聴ページへのリンク）にフォールバックさせる。

---

## 17. `chat.bsky.convo`（Bluesky DM）への自己署名サービス認証【実機疎通確認済み】

seiran は自前 PDS で外部 Bluesky 公式アカウント（bsky.social 等）を一切経由しないため、
Bluesky 公式の DM 機能（`chat.bsky.convo.*`）を呼ぶにも OAuth ではなく §12 の
自己署名 Service Auth JWT を使う。2026-07-20 に実機で疎通確認済み。

### 17-1. `aud` は fragment 無しの素の DID（§12 の video pipeline とは異なるパターン）

`did:web:api.bsky.chat` の DID Document（`https://api.bsky.chat/.well-known/did.json`）:
```json
{"id":"did:web:api.bsky.chat","service":[{"id":"#bsky_chat","type":"BskyChatService","serviceEndpoint":"https://api.bsky.chat"}]}
```

サービス自体は `#bsky_chat` という id を持つが、**JWT の `aud` クレームには fragment を
付けない `"did:web:api.bsky.chat"` を使うのが正解**。fragment込み
（`"did:web:api.bsky.chat#bsky_chat"`）を試すと `401 BadJwtAudience`
（`"jwt audience does not match service did"`）で拒否される。

```rust
// crates/seiran-common/src/atp/service_auth.rs の sign_service_auth_jwt をそのまま使う
let aud = "did:web:api.bsky.chat";  // fragment無し
let lxm = "chat.bsky.convo.listConvos";
let jwt = sign_service_auth_jwt(&pem, &did, aud, lxm)?;
```

§12 の `app.bsky.video.uploadVideo` 実装は `aud` に「呼び出し先サービスではなく
自分の PDS の DID」を渡す特殊パターンだった（動画サービスが後で `uploadBlob` を
代理呼び出しする関係上の設計）。`chat.bsky.convo` は素直な「呼び出し先サービスの
DID（fragment無し）」パターンであり、両者を混同しないこと。

### 17-2. 1対1会話の取得・作成（`getConvoForMembers`）

```
GET https://api.bsky.chat/xrpc/chat.bsky.convo.getConvoForMembers?members=<did1>&members=<did2>
Authorization: Bearer <lxm=chat.bsky.convo.getConvoForMembersで署名したJWT>
```

`members` は1〜10件のDID配列（1:1もグループも同じエンドポイント）。**常に「自分+相手1人」の
2件で呼べば1:1会話に固定できる**（3件以上を渡すとグループ会話になる）。レスポンスの
`convo.kind` が `"directConvo"`/`"groupConvo"` で区別される。

### 17-3. 受信者側のDM許可設定によるビジネスロジック拒否（`NotFollowedBySender`）

```json
{"error":"NotFollowedBySender","message":"recipient requires incoming messages to come from someone they follow"}
```

これは **`400 Bad Request`（`401`ではない）** で返る。JWT認証自体は成功しているが、
受信者側の chat 設定（デフォルトでは多くの場合「フォロー中の相手からのみDM受信」）に
よりビジネスロジックで拒否されている状態。認証方式の疎通確認としては
「401ではなく400/成功が返ること」を確認すれば十分で、このエラー自体は実装の不備ではない。

### 17-4. メッセージ本文の制限（`chat.bsky.convo.defs#messageInput`）

書記素クラスタ数 **1000**・バイト数 **10000**（通常投稿の `app.bsky.feed.post`
300書記素/3000バイトより緩い、別の上限体系）。

### 17-5. `chat.bsky.convo` はJetstreamに乗らない

Jetstream（`wss://jetstream1.us-east.bsky.network/subscribe`）が配信するのは
`app.bsky.feed.post`/`app.bsky.feed.like` 等の公開コレクションのみで、DMは
私信のため一切含まれない。Bsky側からの新着DM受信には `listConvos`/`getMessages` の
定期ポーリングが必須（プッシュ配信の仕組みが無い）。
