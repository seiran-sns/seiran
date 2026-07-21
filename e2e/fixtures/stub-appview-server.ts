// Bsky AppView（public.api.bsky.app）のスタブ実装。E2Eテスト用に ATP_APPVIEW_URL が
// 指す先として起動する。検索・タイムライン同期等が本物のBlueskyへ実通信しないための
// もので、常に空の結果を返すだけでよい（seiranのローカルDB検索結果はこれとは独立して
// 動くため、E2Eで作成したローカル投稿の検索はAppViewが空でも正しく機能する）。
//
// フォロワー検知ポーリング（`bsky_follower_poll`）のE2E検証用に、テストごとに
// `getFollowers` の返す一覧を差し替えられる制御用エンドポイント（`/__control__/followers`）
// を持つ。状態はプロセス内メモリ（`followersByDid`）に保持する。

import { createServer, type Server, type IncomingMessage, type ServerResponse } from "node:http";

export interface StubAppviewServer {
  url: string;
  close(): Promise<void>;
}

interface FollowerEntry {
  did: string;
  handle: string;
  displayName?: string;
  avatar?: string;
}

// targetDid -> フォロワー一覧（新しい順を想定、先頭が最新のフォロワー）。
const followersByDid = new Map<string, FollowerEntry[]>();

function respondJson(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(body));
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf-8")));
    req.on("error", reject);
  });
}

export function startStubAppviewServer(port = 0): Promise<StubAppviewServer> {
  const server: Server = createServer((req, res) => {
    const path = (req.url ?? "").split("?")[0];

    // クエリパラメータ（actor/cursor/limit）が必要なため、この1ケースのみ URL 解析する。
    if (path === "/xrpc/app.bsky.graph.getFollowers" && req.method === "GET") {
      const url = new URL(req.url ?? "", "http://stub");
      const actor = url.searchParams.get("actor") ?? "";
      const limit = Number(url.searchParams.get("limit") ?? "50");
      const cursorParam = url.searchParams.get("cursor");
      const offset = cursorParam ? Number(cursorParam) : 0;

      const all = followersByDid.get(actor) ?? [];
      const page = all.slice(offset, offset + limit);
      const nextOffset = offset + page.length;
      const nextCursor = nextOffset < all.length ? String(nextOffset) : undefined;

      respondJson(res, 200, {
        subject: { did: actor },
        cursor: nextCursor,
        followers: page.map((f) => ({
          did: f.did,
          handle: f.handle,
          displayName: f.displayName,
          avatar: f.avatar,
        })),
      });
      return;
    }

    if (path === "/__control__/followers" && req.method === "POST") {
      readBody(req)
        .then((raw) => {
          const parsed = JSON.parse(raw) as { targetDid: string; followers: FollowerEntry[] };
          followersByDid.set(parsed.targetDid, parsed.followers);
          respondJson(res, 200, { ok: true });
        })
        .catch((e) => {
          respondJson(res, 400, { error: "BadRequest", message: String(e) });
        });
      return;
    }

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
