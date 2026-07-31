import { expect, test } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("「開く」からローカルユーザーIDを解決してプロフィールへ遷移する（#165）", async ({
  page,
  request,
}) => {
  const me = await registerUserViaApi(request, "e2eopenme");
  const target = await registerUserViaApi(request, "e2eopentarget");
  await seedAuth(page, me.token);
  await page.goto("/");

  const navigation = page.getByRole("navigation");
  const openItem = navigation.getByRole("button", { name: /開く/ });
  await expect(
    await openItem.evaluate(
      (element) => element.closest("li")?.previousElementSibling?.textContent,
    ),
  ).toContain("検索");
  await openItem.click();
  await expect(
    page.getByText("URLまたはユーザーID", { exact: true }),
  ).toBeVisible();
  const input = page.getByLabel("開く対象");
  const cameraButton = input.locator("..").getByRole("button", {
    name: "QRコードまたは文字を読み取る",
  });
  await expect(cameraButton).toBeVisible();
  await expect(cameraButton.locator("svg")).toBeVisible();
  await input.fill(`@${target.username}`);
  await page.getByRole("button", { name: "開く", exact: true }).click();

  await expect(page).toHaveURL(new RegExp(`/@${target.username}$`));
  await expect(page.getByText(`@${target.username}`).first()).toBeVisible();
});

test("判別できない入力は「開く」ダイアログ内にエラー表示する（#165）", async ({
  page,
  request,
}) => {
  const me = await registerUserViaApi(request, "e2eopeninvalid");
  await seedAuth(page, me.token);
  await page.goto("/");

  await page.getByRole("button", { name: /開く/ }).click();
  await page.getByLabel("開く対象").fill("not a user or post");
  await page.getByRole("button", { name: "開く", exact: true }).click();

  await expect(page.getByRole("alert")).toHaveText(
    "ユーザーまたはポストではないようです",
  );
});
