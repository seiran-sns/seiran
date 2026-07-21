// ローカルPDSの `com.atproto.sync.subscribeRepos`（`crates/seiran-api/src/handlers/xrpc/sync.rs`
// の `xrpc_subscribe_repos`/`handle_subscribe_repos`）にWebSocketで接続し、配送された
// commitフレームをデコードして特定DIDの `app.bsky.feed.post` レコードを検証するための
// テスト専用クライアント。
//
// フレーム形式（`crates/seiran-common/src/atp/repo.rs` の `build_commit_frame` 参照）:
//   1つのWSバイナリフレーム = dag-cbor(header) ++ dag-cbor(body)（連結、区切りなし）
//   header = {"op": 1, "t": "#commit"}
//   body   = {"did", "ops": [{"action","path","cid"}], "blocks": <CARv1バイト列>, "commit", ...}
//
// `blocks` はCARv1形式（`encode_car`参照）:
//   uvarint(header_len) ++ header_cbor
//   [uvarint(cid_len + block_len) ++ cid_raw_bytes ++ block_bytes] * n
//
// CIDはDAG-CBOR上は tag(42, 0x00 ++ cid.to_bytes()) として現れる（IPLDの標準的な
// CIDリンクエンコーディング、seiran固有ではない）。先頭の0x00（identity multibase由来の
// プレフィックスバイト）を取り除いた残りが CAR 内の生CIDバイト列と一致する。
// 汎用ライブラリ（@ipld/car・multiformats）は使わず、この対応関係だけを頼りに自前でパースする。

import { decode, decodeFirst } from "cborg";
import type { TagDecodeControl } from "cborg";

/** tag 42（CIDリンク）のデコーダ。先頭の0x00プレフィックスを除いた生CIDバイト列を返す。 */
function cidTagDecoder(control: TagDecodeControl): Uint8Array {
  const raw = control() as Uint8Array;
  return raw.subarray(1);
}

const CBOR_DECODE_OPTIONS = { tags: { 42: cidTagDecoder } };

function toHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** LEB128 uvarint を読み、[値, 読み終わった位置] を返す。 */
function readUvarint(buf: Uint8Array, offset: number): [number, number] {
  let result = 0;
  let shift = 0;
  let pos = offset;
  for (;;) {
    const byte = buf[pos];
    pos += 1;
    result |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) break;
    shift += 7;
  }
  return [result >>> 0, pos];
}

/**
 * CIDv1 バイト列（<version:uvarint><codec:uvarint><multihash:<code:uvarint><len:uvarint><digest>>）
 * の先頭から何バイトがCIDかを返す。CARエントリの中で cid と block の境界を割り出すために使う。
 */
function cidByteLength(buf: Uint8Array, offset: number): number {
  let pos = offset;
  [, pos] = readUvarint(buf, pos); // version
  [, pos] = readUvarint(buf, pos); // codec
  [, pos] = readUvarint(buf, pos); // multihash code
  let digestLen: number;
  [digestLen, pos] = readUvarint(buf, pos); // multihash length
  pos += digestLen;
  return pos - offset;
}

/** CARv1バイト列を CID(hex) -> ブロック生バイト列 の Map にパースする。 */
function parseCarBlocks(car: Uint8Array): Map<string, Uint8Array> {
  const blocks = new Map<string, Uint8Array>();
  let pos = 0;

  let headerLen: number;
  [headerLen, pos] = readUvarint(car, pos);
  pos += headerLen; // ヘッダー（roots/version）自体の中身はここでは使わない

  while (pos < car.length) {
    let entryLen: number;
    [entryLen, pos] = readUvarint(car, pos);
    const entryStart = pos;
    const entryEnd = pos + entryLen;
    const cidLen = cidByteLength(car, entryStart);
    const cidBytes = car.subarray(entryStart, entryStart + cidLen);
    const blockBytes = car.subarray(entryStart + cidLen, entryEnd);
    blocks.set(toHex(cidBytes), blockBytes);
    pos = entryEnd;
  }
  return blocks;
}

interface CommitEvtOp {
  action: string;
  path: string;
  cid?: Uint8Array;
}

interface CommitEvtBody {
  did: string;
  ops: CommitEvtOp[];
  blocks: Uint8Array;
  [key: string]: unknown;
}

/**
 * `subscribeRepos` に接続し、対象DIDの `app.bsky.feed.post` create コミットが流れてきて、
 * かつそのレコードの `text` が `expectedText` と一致するイベントが来るまで待つ。
 * `wsUrl` はカーソル省略時=ライブイベントのみ、`?cursor=0` を付けるとDB永続化済みの
 * 過去イベントもリプレイされる（`atp_repo_events`、`find_events_after`）ため接続前の
 * 投稿タイミングを気にせず検証できる。
 *
 * 戻り値: 一致するイベントが見つかれば true、`timeoutMs` 以内に見つからなければ false。
 */
export function waitForBskyFeedPost(
  wsUrl: string,
  targetDid: string,
  expectedText: string,
  timeoutMs = 15_000,
): Promise<boolean> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    ws.binaryType = "arraybuffer";
    let settled = false;

    const finish = (result: boolean) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      try {
        ws.close();
      } catch {
        // クローズ失敗は無視（既に閉じている等）
      }
      resolve(result);
    };

    const timer = setTimeout(() => finish(false), timeoutMs);

    ws.addEventListener("message", (event: MessageEvent) => {
      if (settled) return;
      try {
        const buf = new Uint8Array(event.data as ArrayBuffer);
        const [header, afterHeader] = decodeFirst(buf, {}) as [{ t?: string }, Uint8Array];
        if (header?.t !== "#commit") return;

        const [body] = decodeFirst(afterHeader, CBOR_DECODE_OPTIONS) as [CommitEvtBody, Uint8Array];
        if (body.did !== targetDid) return;

        const createOp = (body.ops ?? []).find(
          (op) => op.action === "create" && typeof op.path === "string" && op.path.startsWith("app.bsky.feed.post/"),
        );
        if (!createOp || !createOp.cid) return;

        const blocks = parseCarBlocks(body.blocks);
        const blockBytes = blocks.get(toHex(createOp.cid));
        if (!blockBytes) return;

        const record = decode(blockBytes, CBOR_DECODE_OPTIONS) as { text?: string };
        if (record.text === expectedText) {
          finish(true);
        }
      } catch {
        // 関係ないフレーム（#identity等）やこのイベントに無関係なブロックのデコード失敗は
        // 無視して次のメッセージを待つ。
      }
    });

    ws.addEventListener("error", () => {
      if (!settled) {
        settled = true;
        clearTimeout(timer);
        reject(new Error("subscribeRepos WebSocket接続エラー"));
      }
    });
  });
}
