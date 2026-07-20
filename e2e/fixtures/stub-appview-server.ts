// Bsky AppView（public.api.bsky.app）のスタブ実装。E2Eテスト用に ATP_APPVIEW_URL が
// 指す先として起動する。検索・タイムライン同期等が本物のBlueskyへ実通信しないための
// もので、常に空の結果を返すだけでよい（seiranのローカルDB検索結果はこれとは独立して
// 動くため、E2Eで作成したローカル投稿の検索はAppViewが空でも正しく機能する）。

import { createServer, type Server } from "node:http";

export interface StubAppviewServer {
  url: string;
  close(): Promise<void>;
}

function respondJson(res: import("node:http").ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(body));
}

export function startStubAppviewServer(port = 0): Promise<StubAppviewServer> {
  const server: Server = createServer((req, res) => {
    const path = (req.url ?? "").split("?")[0];
    switch (path) {
      case "/xrpc/app.bsky.feed.searchPosts":
        respondJson(res, 200, { posts: [], cursor: null });
        return;
      case "/xrpc/app.bsky.feed.getAuthorFeed":
        respondJson(res, 200, { feed: [], cursor: null });
        return;
      case "/xrpc/app.bsky.feed.getPosts":
        respondJson(res, 200, { posts: [] });
        return;
      case "/xrpc/app.bsky.actor.getProfile":
        respondJson(res, 404, { error: "NotFound", message: "profile not found (stub)" });
        return;
      default:
        respondJson(res, 404, { error: "NotFound" });
    }
  });

  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        throw new Error("stub AppView server のアドレス取得に失敗しました");
      }
      resolve({
        url: `http://127.0.0.1:${address.port}`,
        close: () => new Promise((res, rej) => server.close((err) => (err ? rej(err) : res()))),
      });
    });
  });
}

// `node stub-appview-server.ts` で単体起動した場合は APPVIEW_STUB_PORT（デフォルト 2583）で
// 待ち受ける（Playwright の webServer から直接起動するときに使う）。
if (import.meta.url === `file://${process.argv[1]}`) {
  const port = Number(process.env.APPVIEW_STUB_PORT ?? "2583");
  startStubAppviewServer(port).then((stub) => {
    console.log(`stub AppView server listening on ${stub.url}`);
  });
}
