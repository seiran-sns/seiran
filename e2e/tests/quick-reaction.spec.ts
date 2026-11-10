import { test, expect, APIRequestContext, Locator } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// NoteCard の🙂ボタンの右に並ぶクイックリアクションボタン: 先頭は常に❤️、続いて自分が
// よく使う絵文字（1マス幅のもの）を頻度順に最大3つ。

async function createNote(request: APIRequestContext, token: string, text: string) {
  const res = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${token}` },
    data: { text },
  });
  expect(res.ok(), `create failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()) as { id: string };
}

async function react(request: APIRequestContext, token: string, noteId: string, content: string) {
  const res = await request.post(`/api/notes/${noteId}/reactions`, {
    headers: { Authorization: `Bearer ${token}` },
    data: { content },
  });
  expect(res.ok(), `react failed: ${res.status()} ${await res.text()}`).toBeTruthy();
}

/** 1投稿1リアクションなので、使用回数ぶん別々のポストへ付けて「よく使う絵文字」を作る。 */
async function seedFrequentReactions(request: APIRequestContext, token: string, prefix: string) {
  // 👍3・🎉2・❤️2（❤️は固定枠なので重複させない）・🙏1・😂1（同数なら新しい方が上）。
  const usage = ["👍", "👍", "👍", "🎉", "🎉", "❤️", "❤️", "🙏", "😂"];
  for (const [i, emoji] of usage.entries()) {
    const note = await createNote(request, token, `${prefix} 下準備 ${i}`);
    await react(request, token, note.id, emoji);
  }
}

function quickTitles(card: Locator) {
  return card.locator('button[title$=" でリアクション"], button[title$=" のリアクションを取り消す"]');
}

test("❤️のクイックリアクションボタンで付けて、青枠になったボタンで取り消せる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2equickheart");
  const text = `クイックリアクションいいね ${Date.now()}`;
  await createNote(request, user.token, text);

  await seedAuth(page, user.token);
  await page.goto("/");
  const card = page.locator("article").filter({ hasText: text }).first();
  await expect(card).toBeVisible({ timeout: 15_000 });

  // まだ何も使っていないユーザーには❤️だけが並ぶ。
  await expect(quickTitles(card)).toHaveCount(1);
  await card.getByTitle("❤️ でリアクション", { exact: true }).click();

  await expect(card.getByRole("button", { name: /❤️\s*1/ })).toBeVisible({ timeout: 15_000 });
  const removeButton = card.getByTitle("❤️ のリアクションを取り消す", { exact: true });
  await expect(removeButton).toHaveClass(/actionBtnActive/);
  await expect(card.getByTitle("リアクションを付ける", { exact: true })).toHaveClass(/actionBtnActive/);

  await removeButton.click();
  await expect(card.getByRole("button", { name: /❤️\s*\d/ })).toHaveCount(0, { timeout: 15_000 });
  await expect(card.getByTitle("❤️ でリアクション", { exact: true })).not.toHaveClass(/actionBtnActive/);
  await expect(card.getByTitle("リアクションを付ける", { exact: true })).not.toHaveClass(/actionBtnActive/);
});

test("よく使う絵文字が❤️の後ろに頻度順で並び、別の絵文字のボタンで付け替えられる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2equickfreq");
  const prefix = `クイック頻度 ${Date.now()}`;
  await seedFrequentReactions(request, user.token, prefix);
  const text = `${prefix} 対象`;
  await createNote(request, user.token, text);

  await page.setViewportSize({ width: 1600, height: 900 });
  await seedAuth(page, user.token);
  await page.goto("/");
  const card = page.locator("article").filter({ hasText: text }).first();
  await expect(card).toBeVisible({ timeout: 15_000 });

  await expect(quickTitles(card)).toHaveCount(4, { timeout: 15_000 });
  await expect
    .poll(() => quickTitles(card).evaluateAll((els) => els.map((e) => e.getAttribute("title"))))
    .toEqual(["❤️ でリアクション", "👍 でリアクション", "🎉 でリアクション", "😂 でリアクション"]);

  await card.getByTitle("👍 でリアクション", { exact: true }).click();
  await expect(card.getByRole("button", { name: /👍\s*1/ })).toBeVisible({ timeout: 15_000 });
  await expect(card.getByTitle("👍 のリアクションを取り消す", { exact: true })).toHaveClass(/actionBtnActive/);

  // 青枠でないボタンは、付けているリアクションを外してその絵文字で付け直す。
  await card.getByTitle("🎉 でリアクション", { exact: true }).click();
  await expect(card.getByRole("button", { name: /🎉\s*1/ })).toBeVisible({ timeout: 15_000 });
  await expect(card.getByRole("button", { name: /👍\s*\d/ })).toHaveCount(0);
  await expect(card.getByTitle("🎉 のリアクションを取り消す", { exact: true })).toHaveClass(/actionBtnActive/);
  await expect(card.getByTitle("👍 でリアクション", { exact: true })).not.toHaveClass(/actionBtnActive/);
});

test("幅が足りないとクイックリアクションボタンを末尾から隠し、メニューボタンは残す", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2equicknarrow");
  const prefix = `クイック幅 ${Date.now()}`;
  await seedFrequentReactions(request, user.token, prefix);
  const text = `${prefix} 対象`;
  await createNote(request, user.token, text);

  await page.setViewportSize({ width: 360, height: 800 });
  await seedAuth(page, user.token);
  await page.goto("/");
  const card = page.locator("article").filter({ hasText: text }).first();
  await expect(card).toBeVisible({ timeout: 15_000 });

  const menu = card.getByRole("button", { name: "⋯" });
  await expect(menu).toBeVisible();
  await expect(card.getByTitle("😂 でリアクション", { exact: true })).toBeHidden({ timeout: 15_000 });
  const cardBox = await card.boundingBox();
  const menuBox = await menu.boundingBox();
  expect(cardBox && menuBox).toBeTruthy();
  expect(menuBox!.x + menuBox!.width).toBeLessThanOrEqual(cardBox!.x + cardBox!.width);

  // 広げれば全部並ぶ。
  await page.setViewportSize({ width: 1600, height: 900 });
  await expect(card.getByTitle("😂 でリアクション", { exact: true })).toBeVisible({ timeout: 15_000 });
  await expect(menu).toBeVisible();
});
