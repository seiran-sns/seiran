// plc.directory のスタブ実装。E2E テスト用に PLC_DIRECTORY_BASE_URL が指す先として起動する。
// 本物の署名検証・prev チェーン検証は行わず、seiran が送ってきた genesis op をそのまま
// DID ごとに保持して、GET 時に DID ドキュメント形式へ組み直して返すだけ。

import { createServer, type Server } from "node:http";

interface StoredService {
  endpoint: string;
  type: string;
}

interface StoredOp {
  alsoKnownAs: string[];
  prev: string | null;
  rotationKeys: string[];
  services: Record<string, StoredService>;
  sig: string;
  type: string;
  verificationMethods: Record<string, string>;
}

export interface StubPlcServer {
  url: string;
  /** 保存済み genesis op を取得する（テストのアサーション用）。未登録なら undefined。 */
  getOp(did: string): StoredOp | undefined;
  close(): Promise<void>;
}

// did:key:z... から publicKeyMultibase（z... 部分）を取り出す。
// crates/seiran-common/src/atp/plc.rs の p256_to_did_key の逆。
function didKeyToMultibase(didKey: string): string {
  const prefix = "did:key:";
  if (!didKey.startsWith(prefix)) {
    throw new Error(`did:key: 接頭辞がありません: ${didKey}`);
  }
  return didKey.slice(prefix.length);
}

function toDidDocument(did: string, op: StoredOp): object {
  const atprotoKey = op.verificationMethods.atproto;
  const verificationMethod = atprotoKey
    ? [
        {
          id: `${did}#atproto`,
          type: "Multikey",
          controller: did,
          publicKeyMultibase: didKeyToMultibase(atprotoKey),
        },
      ]
    : [];

  const service = Object.entries(op.services).map(([id, svc]) => ({
    id: `#${id}`,
    type: svc.type,
    serviceEndpoint: svc.endpoint,
  }));

  return {
    id: did,
    alsoKnownAs: op.alsoKnownAs,
    verificationMethod,
    service,
  };
}

/** @param port 0 なら空きポートを自動で選ぶ（デフォルト）。固定ポートで待ち受けたい CLI 起動時のみ指定する。 */
export function startStubPlcServer(port = 0): Promise<StubPlcServer> {
  const ops = new Map<string, StoredOp>();

  const server: Server = createServer((req, res) => {
    const did = decodeURIComponent((req.url ?? "/").replace(/^\//, ""));

    if (req.method === "POST" && did) {
      const chunks: Buffer[] = [];
      req.on("data", (chunk) => chunks.push(chunk));
      req.on("end", () => {
        try {
          const op = JSON.parse(Buffer.concat(chunks).toString("utf8")) as StoredOp;
          ops.set(did, op);
          res.writeHead(200, { "content-type": "application/json" }).end("{}");
        } catch (err) {
          res
            .writeHead(400, { "content-type": "application/json" })
            .end(JSON.stringify({ error: "invalid_json", message: String(err) }));
        }
      });
      return;
    }

    if (req.method === "GET" && did) {
      const op = ops.get(did);
      if (!op) {
        res
          .writeHead(404, { "content-type": "application/json" })
          .end(JSON.stringify({ error: "not_found" }));
        return;
      }
      res
        .writeHead(200, { "content-type": "application/json" })
        .end(JSON.stringify(toDidDocument(did, op)));
      return;
    }

    res.writeHead(404, { "content-type": "application/json" }).end(JSON.stringify({ error: "not_found" }));
  });

  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        throw new Error("stub PLC server のアドレス取得に失敗しました");
      }
      resolve({
        url: `http://127.0.0.1:${address.port}`,
        getOp: (did) => ops.get(did),
        close: () => new Promise((res, rej) => server.close((err) => (err ? rej(err) : res()))),
      });
    });
  });
}

// `node stub-plc-server.ts` で単体起動した場合は PLC_STUB_PORT（デフォルト 2582）で待ち受ける
// （Playwright の webServer から直接起動するときに使う）。
if (import.meta.url === `file://${process.argv[1]}`) {
  const port = Number(process.env.PLC_STUB_PORT ?? "2582");
  startStubPlcServer(port).then((stub) => {
    console.log(`stub PLC server listening on ${stub.url}`);
  });
}
