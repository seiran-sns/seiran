import { test, expect } from "@playwright/test";
import { registerUserViaApi } from "../fixtures/api-helpers";

test("ログインするとホームへ遷移する", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2elogin");

  await page.goto("/login");
  await page.getByLabel("メールアドレス / ユーザーネーム").fill(user.username);
  await page.getByLabel("パスワード").fill(user.password);
  await page.getByRole("button", { name: "ログイン" }).click();

  await expect(page).toHaveURL(/\/$/, { timeout: 15_000 });
});

test("パスワードが違うとエラーが表示されログインできない", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2eloginerr");

  await page.goto("/login");
  await page.getByLabel("メールアドレス / ユーザーネーム").fill(user.username);
  await page.getByLabel("パスワード").fill("wrong-password");
  await page.getByRole("button", { name: "ログイン" }).click();

  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByRole("button", { name: "ログイン" })).toBeVisible();
});
