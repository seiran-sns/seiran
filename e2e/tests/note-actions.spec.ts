import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("自分の投稿をピン留め・解除するとメニュー表示が切り替わる", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2epinalice");

  const text = `ピン留めテスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

  await seedAuth(page, alice.token);
  await page.goto("/");

  // ピン留めはケバブメニュー（NoteCardActions の ActionsMenu）内にのみあり、独立ボタンは持たない。
  const card = page.locator("article", { hasText: text });
  const menuTrigger = card.getByRole("button", { name: "⋯" });
  await expect(menuTrigger).toBeVisible({ timeout: 15_000 });

  // メニューを開いて📌項目のラベルを読み、閉じる（トリガー再クリックでトグル閉じる）。
  async function readPinLabel(): Promise<string> {
    await menuTrigger.click();
    const label = (await card.getByRole("button", { name: /^📌/ }).textContent()) ?? "";
    await menuTrigger.click();
    return label.trim();
  }

  await expect.poll(readPinLabel, { timeout: 15_000 }).toBe("📌 ピン留め");
  await menuTrigger.click();
  await card.getByRole("button", { name: "📌 ピン留め" }).click();

  await expect.poll(readPinLabel, { timeout: 10_000 }).toBe("📌 ピン留め解除");
  await menuTrigger.click();
  await card.getByRole("button", { name: "📌 ピン留め解除" }).click();

  await expect.poll(readPinLabel, { timeout: 10_000 }).toBe("📌 ピン留め");
});

test("他人の投稿をリポスト・取り消すとボタン表示が切り替わる", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2erepostalice");
  const bob = await registerUserViaApi(request, "e2erepostbob");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `リポスト対象 ${Date.now()}` },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

  await seedAuth(page, bob.token);
  await page.goto(`/@${alice.username}`);

  const repostButton = page.getByRole("button", { name: /リポスト/ });
  await expect(repostButton).toHaveText("🔁 リポスト", { timeout: 15_000 });

  await repostButton.click();
  await expect(repostButton).toHaveText("🔁 リポスト済み", { timeout: 10_000 });

  await repostButton.click();
  await expect(repostButton).toHaveText("🔁 リポスト", { timeout: 10_000 });
});

test("自分の投稿をケバブメニューから削除でき、確認前はキャンセルできる", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2edeletealice");

  const text = `削除テスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

  await seedAuth(page, alice.token);
  await page.goto("/");

  const card = page.locator("article", { hasText: text });
  const menuTrigger = card.getByRole("button", { name: "⋯" });
  await expect(menuTrigger).toBeVisible({ timeout: 15_000 });

  // キャンセルすると投稿は残る
  await menuTrigger.click();
  await card.getByRole("button", { name: "🗑️ 削除" }).click();
  await expect(page.getByText("投稿を削除しますか？")).toBeVisible({ timeout: 5_000 });
  await page.getByRole("button", { name: "キャンセル" }).click();
  await expect(card).toBeVisible();

  // 確定すると投稿はタイムラインから消える
  await menuTrigger.click();
  await card.getByRole("button", { name: "🗑️ 削除" }).click();
  await page.getByRole("button", { name: "削除する" }).click();
  await expect(card).toHaveCount(0, { timeout: 10_000 });
});
