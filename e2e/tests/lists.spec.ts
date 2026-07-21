import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("リストの作成・改名・メンバー追加/削除・削除ができる", async ({ page, request }) => {
  const owner = await registerUserViaApi(request, "e2elistowner");
  const member = await registerUserViaApi(request, "e2elistmember");
  await seedAuth(page, owner.token);

  await page.goto("/settings/lists");
  await expect(page.getByPlaceholder("新しいリスト名")).toBeVisible({ timeout: 10_000 });

  // 作成
  const listName = `友人 ${Date.now()}`;
  await page.getByPlaceholder("新しいリスト名").fill(listName);
  await page.getByRole("button", { name: "作成", exact: true }).click();

  await expect(page.getByText(listName).first()).toBeVisible({ timeout: 10_000 });
  // 作成直後は自動選択され、編集フォーム（"{name} の編集" 見出し直後のform）にも同じ名前が入る。
  const editHeading = page.getByText(`${listName} の編集`);
  await expect(editHeading).toBeVisible({ timeout: 10_000 });
  const editInput = editHeading.locator("xpath=(following-sibling::form[1]//input)[1]");
  await expect(editInput).toHaveValue(listName);

  // 改名
  const renamed = `${listName}-renamed`;
  await editInput.fill(renamed);
  await page.getByRole("button", { name: "保存", exact: true }).click();
  await expect(page.getByText(`${renamed} の編集`)).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText(renamed).first()).toBeVisible({ timeout: 10_000 });

  // メンバー追加（サジェスト選択を経由せず、target文字列を直接送信する経路を検証）
  await page.getByPlaceholder(/ID\/ハンドル\/ニックネームで検索/).fill(member.username);
  await page.getByRole("button", { name: "メンバー追加" }).click();
  await expect(page.getByText(`@${member.username}`)).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText("1人").first()).toBeVisible({ timeout: 10_000 });

  // メンバー削除
  const memberHandle = page.getByText(`@${member.username}`);
  const memberRow = memberHandle.locator("xpath=ancestor::li[1]");
  await memberRow.getByRole("button", { name: "削除", exact: true }).click();
  await expect(page.getByText("まだメンバーがいません。")).toBeVisible({ timeout: 10_000 });

  // リスト削除（確認ダイアログを承諾）
  page.once("dialog", (d) => d.accept());
  await page.getByRole("button", { name: "削除", exact: true }).click();
  await expect(page.getByText(renamed)).toHaveCount(0, { timeout: 10_000 });
});
