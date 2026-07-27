import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

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

test("起動時の/auth/meが一時的に失敗してもログイン画面に飛ばされず復帰する（#108）", async ({ page, request }) => {
  // バックエンド再起動中の接続断・5xxは認証失効ではないため、AuthContextは
  // トークンを保持したまま少し待ってリトライする(crates/../frontend/src/contexts/authSession.ts)。
  // 最初の1回だけ503を返し、以降は素通りさせて復旧をシミュレートする。
  const user = await registerUserViaApi(request, "e2esess");
  let meCallCount = 0;
  await page.route("**/api/auth/me", (route) => {
    meCallCount += 1;
    if (meCallCount === 1) {
      return route.fulfill({ status: 503, contentType: "application/json", body: "{}" });
    }
    return route.continue();
  });

  await seedAuth(page, user.token);
  await page.goto("/");

  // リトライ(1秒後)で成功しホームに留まる。ログイン画面へリダイレクトされていないことを確認する。
  await expect(page).toHaveURL(/\/$/, { timeout: 15_000 });
  await expect(page).not.toHaveURL(/\/login/);
  expect(meCallCount).toBeGreaterThanOrEqual(2);
});
