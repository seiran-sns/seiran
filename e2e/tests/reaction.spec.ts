import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("投稿にリアクションを付けられる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2ereact");
  await seedAuth(page, user.token);
  await page.goto("/");

  const text = `リアクション対象 ${Date.now()}`;
  await page.getByPlaceholder("いまどうしてる？").fill(text);
  await page.getByRole("button", { name: "投稿", exact: true }).click();
  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });

  await page.getByRole("button", { name: "リアクション" }).click();
  await page.getByPlaceholder("絵文字を検索").fill("thumbs up");
  await page.getByRole("button", { name: "👍", exact: true }).click();

  await expect(page.getByRole("button", { name: /👍/ })).toBeVisible({ timeout: 15_000 });
});
