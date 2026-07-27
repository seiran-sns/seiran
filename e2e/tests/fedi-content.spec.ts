import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

// PR #114（#102）で実装したはずのCW・アンケート表示が、実機確認で全く機能していなかった
// 回帰（フロントのRawNote正規化でcontentWarning/pollが欠落・inbound側でCreate(Question)を
// 無条件無視）を踏まえた再発防止テスト。DBに保存されるだけでなく、実際にNoteCardの
// 表示・トグル挙動まで検証する。

async function findSessionByText(request: import("@playwright/test").APIRequestContext, token: string, text: string) {
  const res = await request.get("/api/dm/sessions", { headers: { Authorization: `Bearer ${token}` } });
  if (!res.ok()) return undefined;
  const sessions = (await res.json()) as { threadRootPostId: string; lastMessage: { text: string } }[];
  return sessions.find((s) => s.lastMessage.text === text);
}

async function findGlobalNoteByText(request: import("@playwright/test").APIRequestContext, token: string, text: string) {
  const res = await request.get("/api/notes/global-timeline?limit=100", {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!res.ok()) return undefined;
  const notes = (await res.json()) as { id: string; text: string }[];
  return notes.find((note) => note.text === text);
}

test.describe("Fediから受信したCW・アンケート付き投稿の表示", () => {
  let fedi: StubFediServer;

  test.beforeEach(async () => {
    fedi = await startStubFediServer();
  });

  test.afterEach(async () => {
    await fedi.close();
  });

  test("CW付き投稿は初期状態で本文が隠れ、ボタンで開閉できる", async ({ page, request }) => {
    const alice = await registerUserViaApi(request, "e2fedicw");
    const text = `CW本文テスト ${Date.now()}`;
    await fedi.sendCreateNote(SEIRAN_BASE_URL, alice.username, text, { summary: "テスト注意書き" });

    let session: { threadRootPostId: string } | undefined;
    await expect
      .poll(
        async () => {
          session = await findSessionByText(request, alice.token, text);
          return session !== undefined;
        },
        { timeout: 15_000 },
      )
      .toBeTruthy();
    const postId = session!.threadRootPostId;

    await seedAuth(page, alice.token);
    await page.goto(`/notes/${postId}`);

    await expect(page.getByText(/テスト注意書き/)).toBeVisible();
    const cwButton = page.getByRole("button", { name: "表示", exact: true });
    await expect(cwButton).toBeVisible();
    await expect(page.getByText(text)).toHaveCount(0);

    await cwButton.click();
    await expect(page.getByText(text)).toBeVisible();

    await page.getByRole("button", { name: "隠す", exact: true }).click();
    await expect(page.getByText(text)).toHaveCount(0);
  });

  test("アンケート付き投稿は選択肢と票数が表示される", async ({ page, request }) => {
    const alice = await registerUserViaApi(request, "e2fedipoll");
    const text = `アンケート本文テスト ${Date.now()}`;
    await fedi.sendCreateNote(SEIRAN_BASE_URL, alice.username, text, {
      pollOptions: ["選択肢A", "選択肢B"],
      visibility: "public",
    });

    let note: { id: string } | undefined;
    await expect
      .poll(
        async () => {
          note = await findGlobalNoteByText(request, alice.token, text);
          return note !== undefined;
        },
        { timeout: 15_000 },
      )
      .toBeTruthy();
    const postId = note!.id;

    await seedAuth(page, alice.token);
    await page.goto(`/notes/${postId}`);

    await expect(page.getByText("選択肢A")).toBeVisible();
    await expect(page.getByText("選択肢B")).toBeVisible();
    await expect(page.getByText("0票")).toHaveCount(0);
    await page.getByRole("button", { name: "結果を見る" }).click();
    await expect(page.getByText("0票").first()).toBeVisible();
    await page.getByRole("button", { name: "回答に戻る" }).click();
    await page.getByRole("button", { name: "選択肢A" }).click();
    await expect(page.getByText("1票")).toBeVisible();

    const repostRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: { renote_id: postId, deliver_to_fedi: false, deliver_to_bsky: false },
    });
    expect(repostRes.ok(), `リポスト作成失敗: ${repostRes.status()} ${await repostRes.text()}`).toBeTruthy();
    const repost = (await repostRes.json()) as { id: string };
    await page.goto(`/notes/${repost.id}`);
    await expect(page.getByText("選択肢A")).toBeVisible();
    await expect(page.getByText("選択肢B")).toBeVisible();
    await page.getByRole("button", { name: "結果を見る" }).click();
    await expect(page.getByText("1票")).toBeVisible();
  });
});
