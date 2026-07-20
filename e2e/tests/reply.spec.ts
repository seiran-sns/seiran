import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// スレッド表示UI（他ユーザーからの返信一覧）は未実装（NoteDetailPage.tsxの
// 「直系リプライ・引用（専用API未実装のためプレースホルダ）」）。そのため返信成功の
// 確認はモーダルが閉じる（=エラーなく投稿完了した）ことをもって行う。
test("他ユーザーの投稿に返信できる", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ereplyauthor");
  const replier = await registerUserViaApi(request, "e2ereplier");

  const originalText = `元投稿 ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: originalText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const original = await createRes.json();

  await seedAuth(page, replier.token);
  await page.goto(`/notes/${original.id}`);
  await expect(page.getByText(originalText)).toBeVisible({ timeout: 15_000 });

  await page.getByRole("button", { name: "返信" }).click();
  const replyText = `返信テスト ${Date.now()}`;
  await page.getByPlaceholder("返信を入力").fill(replyText);
  await page.getByRole("button", { name: "投稿", exact: true }).click();

  // モーダルが閉じ、返信フォームが消える = 投稿成功（エラー時はモーダルが開いたまま）。
  await expect(page.getByPlaceholder("返信を入力")).toHaveCount(0, { timeout: 15_000 });
});
