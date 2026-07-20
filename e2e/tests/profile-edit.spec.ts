import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("プロフィールを編集すると表示名が反映される", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2eprofile");
  await seedAuth(page, user.token);

  await page.goto("/settings/profile");
  const displayName = `E2E表示名${Date.now()}`;
  await page.getByLabel("表示名").fill(displayName);
  await page.getByRole("button", { name: "保存", exact: true }).click();

  // 保存成功後 500ms 後に自分のプロフィールへ遷移する（ProfileEditPage.tsx）。
  await expect(page).toHaveURL(new RegExp(`/@${user.username}$`), { timeout: 15_000 });
  await expect(page.getByText(displayName)).toBeVisible();
});
