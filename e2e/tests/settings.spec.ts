import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("設定メニューからアカウント設定画面へ遷移でき、DIDが表示される", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2eset");
  await seedAuth(page, user.token);

  await page.goto("/settings");
  await expect(page.getByRole("button", { name: "アカウント" })).toBeVisible();

  await page.getByRole("button", { name: "アカウント" }).click();
  await expect(page).toHaveURL(/\/settings\/account$/);
  await expect(page.getByText(/did:/)).toBeVisible({ timeout: 15_000 });
});

test("パスワードを変更でき、新しいパスワードでログインできる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2epw");
  await seedAuth(page, user.token);

  await page.goto("/settings/account");

  // 現在のパスワードが間違っていればエラー。
  await page.getByLabel("現在のパスワード").fill("wrong-password");
  await page.getByLabel("新しいパスワード（8文字以上）").fill("new-password-123");
  await page.getByLabel("新しいパスワード（確認）").fill("new-password-123");
  await page.getByRole("button", { name: "パスワードを変更" }).click();
  await expect(page.getByText("現在のパスワードが正しくありません")).toBeVisible({ timeout: 15_000 });

  // 正しい現在のパスワードなら成功する。
  await page.getByLabel("現在のパスワード").fill("seiranda-e2e");
  await page.getByRole("button", { name: "パスワードを変更" }).click();
  await expect(page.getByText("パスワードを変更しました。")).toBeVisible({ timeout: 15_000 });

  // 新しいパスワードでログインできることを確認する。
  const loginRes = await request.post("/api/auth/login", {
    data: { identifier: user.username, password: "new-password-123" },
  });
  expect(loginRes.ok(), `新パスワードでのログイン失敗: ${loginRes.status()} ${await loginRes.text()}`).toBeTruthy();
});

test("ミュート・ブロック一覧に対象者が表示され、解除するとリストから消える", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2emb1");
  const bob = await registerUserViaApi(request, "e2emb2");

  const muteRes = await request.post("/api/mutes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(muteRes.ok()).toBeTruthy();
  const blockRes = await request.post("/api/blocks/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(blockRes.ok()).toBeTruthy();

  await seedAuth(page, alice.token);
  await page.goto("/settings/mutes-blocks");

  await expect(page.getByText(`@${bob.username}`).first()).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: "ミュート解除" }).click();
  await expect(page.getByText("ミュート中のユーザーはいません。")).toBeVisible({ timeout: 15_000 });

  await page.getByRole("button", { name: "ブロック中" }).click();
  await expect(page.getByText(`@${bob.username}`).first()).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: "ブロック解除" }).click();
  await expect(page.getByText("ブロック中のユーザーはいません。")).toBeVisible({ timeout: 15_000 });
});
