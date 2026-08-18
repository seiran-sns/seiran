import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("投稿にリアクションを付けられる", async ({ page, request }) => {
  // 絵文字ピッカーの初回読み込みが重く、単独実行でも55秒前後かかることがあり
  // デフォルトの60秒制限に対して余裕が無い。E2E並列化でCPU競合が増えると閾値超過
  // しうるため、このテストだけ明示的に延長する。
  test.setTimeout(120_000);

  const user = await registerUserViaApi(request, "e2ereact");
  await seedAuth(page, user.token);
  await page.goto("/");

  const text = `リアクション対象 ${Date.now()}`;
  await page.getByPlaceholder("いまどうしてる？").fill(text);
  await page.getByRole("button", { name: "投稿", exact: true }).click();
  const noteCard = page.locator("article").filter({ hasText: text });
  await expect(noteCard).toBeVisible({ timeout: 15_000 });

  // リアクショントリガーボタンはキャプション文言を持たないため title 属性で特定する。
  await noteCard.getByTitle("リアクションを付ける", { exact: true }).click();
  await page.getByPlaceholder("絵文字を検索").fill("thumbs up");
  await page.getByRole("button", { name: "👍", exact: true }).click();

  await expect(page.getByRole("button", { name: /👍/ })).toBeVisible({ timeout: 15_000 });
});
