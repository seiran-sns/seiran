import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// ポスト詳細画面の右ペイン「リポスト」タブ（#226）: 対象ポストをリポストしたユーザーと
// 時刻の一覧。取り消し済みのリポストも履歴として表示する。
test("ポスト詳細のリポストタブにリポストしたユーザーが表示され、取り消し済みは区別される", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2erepoststabauth");
  const reposter = await registerUserViaApi(request, "e2erepoststabusr");
  const undoer = await registerUserViaApi(request, "e2erepoststabundo");

  const text = `リポストタブ確認用 ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const original = await createRes.json();

  const repostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${reposter.token}` },
    data: { renote_id: original.id },
  });
  expect(repostRes.ok(), `repost failed: ${repostRes.status()} ${await repostRes.text()}`).toBeTruthy();

  const undoRepostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${undoer.token}` },
    data: { renote_id: original.id },
  });
  expect(
    undoRepostRes.ok(),
    `undoer repost failed: ${undoRepostRes.status()} ${await undoRepostRes.text()}`,
  ).toBeTruthy();
  const undoDeleteRes = await request.delete(`/api/notes/${original.id}/repost`, {
    headers: { Authorization: `Bearer ${undoer.token}` },
  });
  expect(
    undoDeleteRes.ok(),
    `undo failed: ${undoDeleteRes.status()} ${await undoDeleteRes.text()}`,
  ).toBeTruthy();

  await seedAuth(page, author.token);
  await page.goto(`/notes/${original.id}`);
  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });

  const rightPane = page.getByRole("complementary");
  await rightPane.getByRole("button", { name: "リポスト", exact: true }).click();

  // 有効なリポストはクリック可能なリンク（<a>）として描画される。
  await expect(rightPane.locator("a").filter({ hasText: reposter.username })).toHaveCount(1);
  // 取り消し済みは一覧には出るが詳細画面が無いためリンク化されない。
  await expect(rightPane.getByText(undoer.username)).toBeVisible({ timeout: 15_000 });
  await expect(rightPane.locator("a").filter({ hasText: undoer.username })).toHaveCount(0);
  await expect(rightPane.getByText("取り消し済み")).toBeVisible();
});
