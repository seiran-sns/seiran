import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("通知一覧を通知種別とユーザーで絞り込める（両方指定はAND）", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2enfilteralice");
  const bob = await registerUserViaApi(request, "e2enfilterbob");
  const carol = await registerUserViaApi(request, "e2enfiltercarol");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `絞り込みテスト ${Date.now()}` },
  });
  expect(createRes.ok(), await createRes.text()).toBeTruthy();
  const note = await createRes.json();

  for (const reactor of [bob, carol]) {
    const res = await request.post(`/api/notes/${note.id}/reactions`, {
      headers: { Authorization: `Bearer ${reactor.token}` },
      data: { content: "🎉" },
    });
    expect(res.ok(), await res.text()).toBeTruthy();
  }
  const follow = await request.post("/api/follows/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { target: alice.username },
  });
  expect(follow.ok(), await follow.text()).toBeTruthy();

  await seedAuth(page, alice.token);
  await page.goto("/notifications");

  const bobReaction = page.getByText(`${bob.username} がリアクションしました`);
  const carolReaction = page.getByText(`${carol.username} がリアクションしました`);
  const bobFollow = page.getByText(`${bob.username} にフォローされました`);
  await expect(bobReaction).toBeVisible({ timeout: 15_000 });
  await expect(carolReaction).toBeVisible();
  await expect(bobFollow).toBeVisible();

  // 通知種別: 「リアクション」だけに絞るとフォロー通知が消える。
  await page.getByRole("button", { name: "通知種別で絞り込み" }).click();
  await page.getByLabel("リアクション", { exact: true }).check();
  await expect(bobFollow).toBeHidden();
  await expect(bobReaction).toBeVisible();
  await expect(carolReaction).toBeVisible();
  await page.keyboard.press("Escape");

  // ユーザー: bobを指定すると、種別との AND でbobのリアクションだけが残る。
  await page.getByRole("button", { name: "ユーザーで絞り込み" }).click();
  await page.getByPlaceholder(/^ID\/ハンドル/).fill(bob.username);
  await page.getByRole("button", { name: new RegExp(`@${bob.username}`) }).click();
  await expect(carolReaction).toBeHidden();
  await expect(bobReaction).toBeVisible();
  await expect(bobFollow).toBeHidden();
  await page.keyboard.press("Escape");

  // 種別のチェックを外す（未指定）とbobのフォロー通知も戻る。全種別を選んだ場合も同じ意味。
  await page.getByRole("button", { name: "通知種別で絞り込み" }).click();
  await page.getByLabel("リアクション", { exact: true }).uncheck();
  await expect(bobFollow).toBeVisible();
  await expect(carolReaction).toBeHidden();

  // ✕ボタンでユーザー指定を全部外すと、carolの通知も戻る。
  await page.getByRole("button", { name: "すべてクリア" }).click();
  await expect(carolReaction).toBeVisible();
  await expect(page.getByRole("button", { name: "すべてクリア" })).toBeHidden();
});
