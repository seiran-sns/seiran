// S3互換ストレージのスタブ実装。SigV4署名検証等は行わず、PUTされたオブジェクトを
// メモリ上に保持しGETで返すだけの最小実装。E2E環境には実際のS3/MinIOが無いため、
// 画像アップロードを伴うテスト（カスタム絵文字登録等）でのみ`force_path_style`な
// エンドポイントとして起動する。

import { createServer, type Server } from "node:http";

export interface StubS3Server {
  url: string;
  close(): Promise<void>;
}

export async function startStubS3Server(): Promise<StubS3Server> {
  const objects = new Map<string, Buffer>();

  const server: Server = createServer((req, res) => {
    const key = req.url ?? "/";
    if (req.method === "PUT") {
      const chunks: Buffer[] = [];
      req.on("data", (c) => chunks.push(c));
      req.on("end", () => {
        objects.set(key, Buffer.concat(chunks));
        res.writeHead(200, { etag: '"stub"' });
        res.end();
      });
      return;
    }
    if (req.method === "GET" || req.method === "HEAD") {
      const body = objects.get(key);
      if (!body) {
        res.writeHead(404);
        res.end();
        return;
      }
      res.writeHead(200, { "content-length": body.length });
      res.end(req.method === "GET" ? body : undefined);
      return;
    }
    res.writeHead(405);
    res.end();
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("stub-s3-server: アドレス取得失敗");
  }

  return {
    url: `http://127.0.0.1:${address.port}`,
    close: () => new Promise<void>((resolve, reject) => server.close((e) => (e ? reject(e) : resolve()))),
  };
}
