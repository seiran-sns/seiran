import { test, expect } from "@playwright/test";

// 既存DID転入フロー（docs/account_migration.md）の入口画面のUIレベル検証。
// 実際のBluesky（PDS A）との通信は行わない。ハンドル解決失敗時のエラー表示確認のみ、
// RFC 2606で名前解決されないことが保証された .invalid ドメインを使い、実在のBluesky
// サービスには一切到達させない（DNS解決自体が失敗するため）。

test("通常登録画面から移行導線で遷移でき、移行フォームが表示される", async ({ page }) => {
  await page.goto("/register");
  await page.getByRole("link", { name: /移行/ }).click();

  await expect(page).toHaveURL(/\/register\/migrate$/);
  await expect(page.getByLabel("移行元のハンドル")).toBeVisible();
  await expect(page.getByLabel("移行元のパスワード")).toBeVisible();
  await expect(page.getByLabel("seiranでの新しいユーザー名")).toBeVisible();
  await expect(page.getByLabel("seiranでの新しいパスワード（移行元とは別に設定してください）")).toBeVisible();
});

test("通常の新規登録に戻るリンクで/registerへ戻れる", async ({ page }) => {
  await page.goto("/register/migrate");
  await page.getByRole("link", { name: "通常の新規登録に戻る" }).click();
  await expect(page).toHaveURL(/\/register$/);
});

test("必須項目未入力では送信できない（ブラウザバリデーション）", async ({ page }) => {
  await page.goto("/register/migrate");
  await page.getByRole("button", { name: "移行を開始" }).click();
  // ネイティブのrequiredバリデーションでフォーム送信自体が止まり、画面遷移しない。
  await expect(page).toHaveURL(/\/register\/migrate$/);
});

test("移行元ハンドルが解決できない場合エラーメッセージを表示する", async ({ page }) => {
  const suffix = Date.now().toString(36);

  await page.goto("/register/migrate");
  await page.getByLabel("移行元のハンドル").fill("nonexistent-e2e-test.invalid");
  await page.getByLabel("移行元のパスワード").fill("dummy-password");
  await page.getByLabel("seiranでの新しいユーザー名").fill(`e2emig${suffix}`);
  await page.getByLabel("seiranでの新しいパスワード（移行元とは別に設定してください）").fill("seiranda-e2e");
  const emailField = page.getByLabel("メールアドレス");
  if (await emailField.isVisible().catch(() => false)) {
    await emailField.fill(`e2e-mig-${suffix}@example.com`);
  }
  await page.getByRole("button", { name: "移行を開始" }).click();

  await expect(page.getByText("移行元のハンドルを解決できませんでした")).toBeVisible({ timeout: 15_000 });
  await expect(page).toHaveURL(/\/register\/migrate$/);
});
