import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("投稿するとタイムラインに表示される", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2epost");
  await seedAuth(page, user.token);

  await page.goto("/");

  const text = `E2Eテスト投稿 ${Date.now()}`;
  await page.getByPlaceholder("いまどうしてる？").fill(text);
  await page.getByRole("button", { name: "投稿", exact: true }).click();

  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });
});
