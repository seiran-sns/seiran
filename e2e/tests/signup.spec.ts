import { test, expect } from "@playwright/test";

// 新規登録はバックエンドで同期的に did:plc genesis（DID確定 → PLCディレクトリへ送信）
// まで行う（crates/seiran-api/src/handlers/auth.rs の register）。ここが失敗すると
// /api/auth/register は 500 を返し画面にエラーが出るため、"/" へのリダイレクトが
// 確認できればスタブPLCサーバーとの疎通も含めて成功している。
test("新規登録するとPLC genesisを経てタイムラインへ遷移する", async ({ page }) => {
  const suffix = Date.now().toString(36);
  const email = `e2e-${suffix}@example.com`;
  const username = `e2e${suffix}`;
  const password = "seiranda-e2e";

  await page.goto("/register");

  await page.getByLabel("メールアドレス").fill(email);
  await page.getByLabel("ユーザー名").fill(username);
  await page.getByLabel("パスワード（8文字以上）").fill(password);
  await page.getByRole("button", { name: "登録する" }).click();

  await expect(page).toHaveURL(/\/$/, { timeout: 15_000 });
  await expect(page.getByText(/登録に失敗|エラー/)).toHaveCount(0);
});
