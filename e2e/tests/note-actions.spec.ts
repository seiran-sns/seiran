import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("自分の投稿をピン留め・解除するとボタン表示が切り替わる", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2epinalice");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `ピン留めテスト ${Date.now()}` },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

  await seedAuth(page, alice.token);
  await page.goto("/");

  const pinButton = page.getByRole("button", { name: /ピン留め/ });
  await expect(pinButton).toHaveText("📌 ピン留め", { timeout: 15_000 });

  await pinButton.click();
  await expect(pinButton).toHaveText("📌 ピン留め済み", { timeout: 10_000 });

  await pinButton.click();
  await expect(pinButton).toHaveText("📌 ピン留め", { timeout: 10_000 });
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
