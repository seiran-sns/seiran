import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("ローカルユーザーをフォローすると即座にフォロー中になる", async ({ page, request }) => {
  // ローカルユーザー同士のフォローは承認不要で即accepted
  // (crates/seiran-api/src/handlers/follows.rs)。
  const follower = await registerUserViaApi(request, "e2efol");
  const target = await registerUserViaApi(request, "e2etgt");
  await seedAuth(page, follower.token);

  await page.goto(`/@${target.username}`);
  // フォローボタンはユーザーメニュー内にも統合されているが、独立ボタンとしても
  // followArea に残しているため、メニューを開かず直接クリックできる。
  await page.getByRole("button", { name: "フォロー", exact: true }).click();

  await expect(page.getByText("フォロー中")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("button", { name: "フォロー解除" })).toBeVisible();
});
