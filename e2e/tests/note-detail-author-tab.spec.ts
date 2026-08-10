import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// ポスト詳細画面の右ペイン「投稿者」タブ（#226）: プロフィール概要と固定ポストを表示する。
test("ポスト詳細の投稿者タブにプロフィールと固定ポストが表示される", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2eauthortab");
  const viewer = await registerUserViaApi(request, "e2eauthorviewer");

  const pinnedText = `固定ポスト ${Date.now()}`;
  const pinnedRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: pinnedText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(pinnedRes.ok(), `create failed: ${pinnedRes.status()} ${await pinnedRes.text()}`).toBeTruthy();
  const pinnedNote = await pinnedRes.json();

  const pinRes = await request.post(`/api/notes/${pinnedNote.id}/pin`, {
    headers: { Authorization: `Bearer ${author.token}` },
  });
  expect(pinRes.ok(), `pin failed: ${pinRes.status()} ${await pinRes.text()}`).toBeTruthy();

  const mainText = `詳細確認用ポスト ${Date.now()}`;
  const mainRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: mainText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(mainRes.ok(), `create failed: ${mainRes.status()} ${await mainRes.text()}`).toBeTruthy();
  const mainNote = await mainRes.json();

  await seedAuth(page, viewer.token);
  await page.goto(`/notes/${mainNote.id}`);
  await expect(page.getByText(mainText)).toBeVisible({ timeout: 15_000 });

  await page.getByRole("button", { name: "投稿者", exact: true }).click();
  const rightPane = page.getByRole("complementary");
  await expect(rightPane.getByText(`@${author.username}`).first()).toBeVisible({ timeout: 15_000 });
  await expect(rightPane.getByText(pinnedText)).toBeVisible({ timeout: 15_000 });
});
