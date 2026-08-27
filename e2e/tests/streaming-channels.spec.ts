import { test, expect } from "@playwright/test";
import WebSocket from "ws";
import { registerUserViaApi } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_PORT, BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

// Misskey互換のタイムラインチャンネル購読（connect/channel/disconnect）のE2E。
// ローカルタイムライン表示中にリモートフォロー先の投稿が混入していたバグ（#通知調査時に発見）
// の直接的な回帰テストと、リストチャンネル・disconnectの動作確認を行う。
// CIはNode 20固定（グローバルWebSocket非搭載）のため、`ws`パッケージを使う
// （`fixtures/subscribe-repos-client.ts`と同じ方式）。

interface ChannelEvent {
  type: string;
  body: { id: string; type: string; body: { text?: string } };
}

function openStream(token: string): { ws: WebSocket; received: ChannelEvent[] } {
  const ws = new WebSocket(`ws://localhost:${BACKEND_PORT}/api/streaming?token=${encodeURIComponent(token)}`);
  const received: ChannelEvent[] = [];
  ws.addEventListener("message", (ev) => {
    try {
      received.push(JSON.parse(ev.data as string));
    } catch {
      /* 無視 */
    }
  });
  return { ws, received };
}

async function waitOpen(ws: WebSocket): Promise<void> {
  if (ws.readyState === WebSocket.OPEN) return;
  await new Promise<void>((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", () => reject(new Error("WebSocket接続に失敗しました")));
  });
}

function connectChannel(ws: WebSocket, channel: string, id: string, params: Record<string, unknown> = {}) {
  ws.send(JSON.stringify({ type: "connect", body: { channel, id, params } }));
}

function disconnectChannel(ws: WebSocket, id: string) {
  ws.send(JSON.stringify({ type: "disconnect", body: { id } }));
}

function hasChannelNote(received: ChannelEvent[], subId: string, text: string): boolean {
  return received.some(
    (m) => m.type === "channel" && m.body?.id === subId && m.body?.type === "note" && m.body?.body?.text === text
  );
}

async function expectChannelNoteArrives(received: ChannelEvent[], subId: string, text: string) {
  await expect.poll(() => hasChannelNote(received, subId, text), { timeout: 15_000 }).toBeTruthy();
}

async function expectChannelNoteAbsent(received: ChannelEvent[], subId: string, text: string, waitMs = 3000) {
  await new Promise((r) => setTimeout(r, waitMs));
  expect(hasChannelNote(received, subId, text)).toBe(false);
}

test.describe("WebSocketタイムラインチャンネル購読", () => {
  test("ローカルタイムラインチャンネルにはリモート投稿が混入せず、グローバルタイムラインには届く", async ({
    request,
  }) => {
    const viewer = await registerUserViaApi(request, "e2estreamltl");
    const fedi = await startStubFediServer();
    try {
      const { ws, received } = openStream(viewer.token);
      try {
        await waitOpen(ws);
        connectChannel(ws, "localTimeline", "ltl");
        connectChannel(ws, "globalTimeline", "gtl");

        const text = `streaming channel remote test ${Date.now()}`;
        await fedi.sendCreateNote(SEIRAN_BASE_URL, viewer.username, text, { visibility: "public" });

        // グローバルには届く（リモート投稿でも is_local=false 条件なしで一致する）。
        await expectChannelNoteArrives(received, "gtl", text);
        // ローカルタイムラインには is_local=false のため一致しない
        // （これが混入していたら今回のバグの回帰）。
        await expectChannelNoteAbsent(received, "ltl", text);
      } finally {
        ws.close();
      }
    } finally {
      await fedi.close();
    }
  });

  test("リストチャンネルはそのリストのメンバーの新規投稿だけが届く", async ({ request }) => {
    const owner = await registerUserViaApi(request, "e2estreamlistown");
    const member = await registerUserViaApi(request, "e2estreamlistmem");
    const other = await registerUserViaApi(request, "e2estreamlistother");

    const createList = await request.post("/api/lists", {
      headers: { Authorization: `Bearer ${owner.token}` },
      data: { name: `E2E list ${Date.now()}`, is_public: false },
    });
    expect(createList.ok(), `list作成失敗: ${createList.status()} ${await createList.text()}`).toBeTruthy();
    const list = (await createList.json()) as { id: string };

    const addMember = await request.post(`/api/lists/${list.id}/members`, {
      headers: { Authorization: `Bearer ${owner.token}` },
      data: { target: member.username },
    });
    expect(addMember.ok(), `メンバー追加失敗: ${addMember.status()} ${await addMember.text()}`).toBeTruthy();

    const { ws, received } = openStream(owner.token);
    try {
      await waitOpen(ws);
      connectChannel(ws, "userList", "mylist", { listId: list.id });

      const memberText = `list member post ${Date.now()}`;
      const memberPost = await request.post("/api/notes/create", {
        headers: { Authorization: `Bearer ${member.token}` },
        data: { text: memberText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
      });
      expect(memberPost.ok(), `投稿失敗: ${memberPost.status()} ${await memberPost.text()}`).toBeTruthy();
      await expectChannelNoteArrives(received, "mylist", memberText);

      const otherText = `non member post ${Date.now()}`;
      const otherPost = await request.post("/api/notes/create", {
        headers: { Authorization: `Bearer ${other.token}` },
        data: { text: otherText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
      });
      expect(otherPost.ok(), `投稿失敗: ${otherPost.status()} ${await otherPost.text()}`).toBeTruthy();
      await expectChannelNoteAbsent(received, "mylist", otherText);
    } finally {
      ws.close();
    }
  });

  test("ホームタイムラインチャンネルはフォロー中ユーザーのリプライでもリプライ先未フォローなら届かない", async ({
    request,
  }) => {
    const viewer = await registerUserViaApi(request, "e2estreamhtlviewer");
    const followed = await registerUserViaApi(request, "e2estreamhtlfollowed");
    const unfollowed = await registerUserViaApi(request, "e2estreamhtlunfollowed");

    const follow = await request.post("/api/follows/create", {
      headers: { Authorization: `Bearer ${viewer.token}` },
      data: { target: followed.username },
    });
    expect(follow.ok(), `follow failed: ${follow.status()} ${await follow.text()}`).toBeTruthy();

    const unfollowedPost = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${unfollowed.token}` },
      data: {
        text: `htl reply base post ${Date.now()}`,
        deliver_to_fedi: false,
        deliver_to_bsky: false,
        visibility: "public",
      },
    });
    expect(unfollowedPost.ok()).toBeTruthy();
    const unfollowedPostBody = (await unfollowedPost.json()) as { id: string };

    const { ws, received } = openStream(viewer.token);
    try {
      await waitOpen(ws);
      connectChannel(ws, "homeTimeline", "htl");

      const replyText = `htl reply to unfollowed ${Date.now()}`;
      const replyPost = await request.post("/api/notes/create", {
        headers: { Authorization: `Bearer ${followed.token}` },
        data: {
          text: replyText,
          deliver_to_fedi: false,
          deliver_to_bsky: false,
          visibility: "public",
          reply_to_id: unfollowedPostBody.id,
        },
      });
      expect(replyPost.ok(), `reply failed: ${replyPost.status()} ${await replyPost.text()}`).toBeTruthy();
      await expectChannelNoteAbsent(received, "htl", replyText);

      const normalText = `htl normal followed post ${Date.now()}`;
      const normalPost = await request.post("/api/notes/create", {
        headers: { Authorization: `Bearer ${followed.token}` },
        data: { text: normalText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
      });
      expect(normalPost.ok()).toBeTruthy();
      await expectChannelNoteArrives(received, "htl", normalText);
    } finally {
      ws.close();
    }
  });

  test("disconnect後は該当チャンネルのイベントが届かなくなる", async ({ request }) => {
    const viewer = await registerUserViaApi(request, "e2estreamdisconnect");

    const { ws, received } = openStream(viewer.token);
    try {
      await waitOpen(ws);
      connectChannel(ws, "localTimeline", "ltl");

      const beforeText = `before disconnect ${Date.now()}`;
      const beforePost = await request.post("/api/notes/create", {
        headers: { Authorization: `Bearer ${viewer.token}` },
        data: { text: beforeText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
      });
      expect(beforePost.ok()).toBeTruthy();
      await expectChannelNoteArrives(received, "ltl", beforeText);

      disconnectChannel(ws, "ltl");
      // disconnect が処理される猶予を与える。
      await new Promise((r) => setTimeout(r, 500));

      const afterText = `after disconnect ${Date.now()}`;
      const afterPost = await request.post("/api/notes/create", {
        headers: { Authorization: `Bearer ${viewer.token}` },
        data: { text: afterText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
      });
      expect(afterPost.ok()).toBeTruthy();
      await expectChannelNoteAbsent(received, "ltl", afterText);
    } finally {
      ws.close();
    }
  });
});
