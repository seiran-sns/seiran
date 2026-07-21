import { test, expect } from "@playwright/test";
import { loginViaApi, registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// global-setup.ts が作成する初期管理者アカウント（role=admin）。
const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e";

test("管理者以外が/adminにアクセスするとホームへリダイレクトされる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2eadminguard");
  await seedAuth(page, user.token);

  await page.goto("/admin");

  await expect(page).toHaveURL(/\/$/, { timeout: 10_000 });
});

test("管理者はサイト設定を変更でき、リロード後も反映されている", async ({ page, request }) => {
  const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  await seedAuth(page, adminToken);

  await page.goto("/admin");
  // 既定タブ（ユーザー管理）が描画されるまで待ってから「サイト設定」タブへ切り替える。
  await expect(page.getByText("ユーザー管理")).toBeVisible({ timeout: 10_000 });
  await page.getByRole("button", { name: "サイト設定" }).click();

  const newName = `seiran-e2e-${Date.now()}`;
  const nameInput = page.getByLabel("サイト名称");
  await nameInput.fill(newName);
  await page.getByRole("button", { name: "保存" }).click();

  await expect(page.getByText("保存しました。")).toBeVisible({ timeout: 10_000 });

  await page.reload();
  await expect(page.getByText("ユーザー管理")).toBeVisible({ timeout: 10_000 });
  await page.getByRole("button", { name: "サイト設定" }).click();
  await expect(page.getByLabel("サイト名称")).toHaveValue(newName, { timeout: 10_000 });
});

test("管理者はユーザーを凍結・凍結解除できる", async ({ page, request }) => {
  const target = await registerUserViaApi(request, "e2eadmintarget");
  const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  await seedAuth(page, adminToken);

  await page.goto("/admin");
  await expect(page.getByText("ユーザー管理")).toBeVisible({ timeout: 10_000 });

  // ユーザー一覧の行構造（UserManagement.tsx）: primaryText(@username) → grow → row
  // の2階層上に、凍結ボタン・ロール選択を含む行全体(row)がある。
  const usernameCell = page.getByText(`@${target.username}`, { exact: true });
  await expect(usernameCell).toBeVisible({ timeout: 10_000 });
  const row = usernameCell.locator("xpath=../..");

  await row.getByRole("button", { name: "凍結", exact: true }).click();
  await expect(row.getByText("凍結中")).toBeVisible({ timeout: 10_000 });

  await row.getByRole("button", { name: "凍結解除" }).click();
  await expect(row.getByText("凍結中")).toHaveCount(0, { timeout: 10_000 });
});
