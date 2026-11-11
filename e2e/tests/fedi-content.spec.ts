import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

// Fediから受信したCW・アンケートを、DBへの保存だけでなくNoteCardの表示・トグル挙動まで検証する
// （フロントの正規化や受信側の型判定で落ちても気づけるように）。

async function findSessionByText(request: import("@playwright/test").APIRequestContext, token: string, text: string) {
  const res = await request.get("/api/dm/sessions", { headers: { Authorization: `Bearer ${token}` } });
  if (!res.ok()) return undefined;
  const sessions = (await res.json()) as { threadRootPostId: string; lastMessage: { text: string } }[];
  return sessions.find((s) => s.lastMessage.text === text);
}

// GTLは並列実行中の他specの投稿も乗るため、直近1ページ（100件）だけを見ると
// 探している投稿が押し出されてflakyになりうる。until_idページネーションで
// 数ページ分（最大500件）まで遡って探す。
async function findGlobalNoteByText(request: import("@playwright/test").APIRequestContext, token: string, text: string) {
  let untilId: string | undefined;
  for (let page = 0; page < 5; page++) {
    const url = untilId
      ? `/api/notes/global-timeline?limit=100&until_id=${untilId}`
      : "/api/notes/global-timeline?limit=100";
    const res = await request.get(url, {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!res.ok()) return undefined;
    const notes = (await res.json()) as { id: string; text: string }[];
    const found = notes.find((note) => note.text === text);
    if (found) return found;
    if (notes.length < 100) return undefined; // 最終ページまで見終えた
    untilId = notes[notes.length - 1].id;
  }
  return undefined;
}

test.describe("Fediから受信したCW・アンケート付き投稿の表示", () => {
  let fedi: StubFediServer;

  test.beforeEach(async () => {
    fedi = await startStubFediServer();
  });

  test.afterEach(async () => {
    await fedi.close();
  });

  test("DMのCW付きメッセージは初期状態で本文が隠れ、ボタンで開閉できる", async ({ page, request }) => {
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

    // DM扱いのため右ペインのセッション一覧にも同じCW注意書きがプレビュー表示される
    // （こちらは本文の代わりにCWラベルのみ表示）。主投稿の開閉挙動を
    // 検証したいこのテストでは main 領域に絞ってロケーターの曖昧さを避ける。
    const main = page.locator("main");
    await expect(main.getByText(/テスト注意書き/)).toBeVisible();
    const cwButton = main.getByRole("button", { name: "表示", exact: true });
    await expect(cwButton).toBeVisible();
    await expect(main.getByText(text)).toHaveCount(0);

    await cwButton.click();
    await expect(main.getByText(text)).toBeVisible();

    await main.getByRole("button", { name: "隠す", exact: true }).click();
    await expect(main.getByText(text)).toHaveCount(0);
  });

  test("CW付き投稿は本文が隠れ「全部表示」で開く。警告文のタグ・MFMは変換して表示する", async ({ page, request }) => {
    const alice = await registerUserViaApi(request, "e2fedicwp");
    const text = `CW公開本文テスト ${Date.now()}`;
    await fedi.sendCreateNote(SEIRAN_BASE_URL, alice.username, text, {
      summary: "<small>テスト</small>注意書き $[ruby 鬼徹 コイツ] <script>x</script><ご報告>",
      visibility: "public",
    });

    let postId: string | undefined;
    await expect
      .poll(
        async () => {
          postId = (await findGlobalNoteByText(request, alice.token, text))?.id;
          return postId !== undefined;
        },
        { timeout: 15_000 },
      )
      .toBeTruthy();

    await seedAuth(page, alice.token);
    await page.goto(`/notes/${postId}`);

    const card = page.locator("main article").first();
    await expect(card.getByText(/注意書き/)).toBeVisible();
    // 警告文のタグは描画され、文字としては見えない。MFMのルビは<ruby>になる。
    await expect(card.locator("small", { hasText: "テスト" })).toBeVisible();
    await expect(card.locator("ruby", { hasText: "鬼徹" })).toBeVisible();
    await expect(card.getByText(/<small>|\$\[ruby/)).toHaveCount(0);
    // タグの形をしていない`<`は文字のまま残る。
    await expect(card.getByText(/<ご報告>/)).toBeVisible();

    await expect(card.getByRole("button", { name: "表示", exact: true })).toHaveCount(0);
    const showAll = card.getByRole("button", { name: "全部表示", exact: true });
    await expect(showAll).toBeVisible();
    await expect(card.getByText(text)).toHaveCount(0);

    await showAll.click();
    await expect(card.getByText(text)).toBeVisible();
    await expect(showAll).toHaveCount(0);
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
    await expect(page.getByText("✓ 選択肢A")).toBeVisible();

    await page.reload();
    await expect(page.getByText("1票")).toBeVisible();
    await expect(page.getByText("✓ 選択肢A")).toBeVisible();
    await expect(page.getByRole("button", { name: "選択肢A" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "回答に戻る" })).toHaveCount(0);

    const repostRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: { renote_id: postId, deliver_to_fedi: false, deliver_to_bsky: false },
    });
    expect(repostRes.ok(), `リポスト作成失敗: ${repostRes.status()} ${await repostRes.text()}`).toBeTruthy();
    const repost = (await repostRes.json()) as { id: string };
    await page.goto(`/notes/${repost.id}`);
    await expect(page.getByText("✓ 選択肢A")).toBeVisible();
    await expect(page.getByText("選択肢B")).toBeVisible();
    await expect(page.getByText("1票")).toBeVisible();
  });
});
