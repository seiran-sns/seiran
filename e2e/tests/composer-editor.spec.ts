import { expect, test } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("投稿本文のメンション候補をキーボードで選択し既知IDとして表示できる", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposer");
  const target = await registerUserViaApi(request, "e2ecomposertarget");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.fill(`@${target.username.slice(0, -2)}`);

  const option = page.getByRole("option").filter({ hasText: `@${target.username}` });
  await expect(option).toBeVisible();
  await editor.press("Enter");

  await expect(editor).toContainText(`@${target.username}`);
  const mention = editor.locator("span", { hasText: `@${target.username}` });
  await expect(mention).toHaveCSS("font-weight", "700");
  await expect(mention).toHaveCSS("color", "rgb(22, 139, 210)");
});

test("ローカルユーザーは短いIDで候補表示し、3形式のIDを既知として扱う", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposerlocal");
  const target = await registerUserViaApi(request, "e2ecomposerlocaltarget");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.fill(`@${target.username.slice(0, -2)}`);
  const option = page.getByRole("option").filter({ hasText: target.username });
  await expect(option).toBeVisible();
  await expect(option.locator("small")).toHaveText(`@${target.username}`);

  const search = await request.get(`/api/actors/search?q=${target.username}`, {
    headers: { Authorization: `Bearer ${author.token}` },
  });
  const actors = (await search.json()) as { username: string; domain: string }[];
  const local = actors.find((actor) => actor.username === target.username)!;
  for (const mention of [
    `@${target.username}`,
    `@${target.username}.${local.domain}`,
    `@${target.username}@${local.domain}`,
  ]) {
    await editor.fill(mention);
    await expect(editor.locator("span", { hasText: mention })).toHaveCSS("color", "rgb(22, 139, 210)");
  }
});

test("カスタム絵文字候補を画像へ置換し境界Backspaceで通常テキストへ戻せる", async ({ page, request }) => {
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  const author = await registerUserViaApi(request, "e2ecomposeremoji");
  await page.route("**/api/emojis", (route) =>
    route.fulfill({
      json: {
        emojis: [{
          id: "1",
          aliases: ["long"],
          name: "wide_emoji",
          category: null,
          host: null,
          url: "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='80' height='20'/%3E",
          license: null,
        }],
      },
    })
  );
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await expect(editor).toBeVisible();
  await editor.click();
  await editor.pressSequentially(":wide");
  await expect(page.getByRole("option", { name: /:wide_emoji:/ })).toBeVisible();
  await editor.press("ArrowDown");
  await editor.press("Enter");

  const emoji = editor.getByRole("img", { name: ":wide_emoji:" });
  await expect(emoji).toBeVisible();
  await expect(emoji).toHaveCSS("height", "24px");
  await expect(page.getByRole("listbox", { name: "入力候補" })).toHaveCount(0);

  await editor.press("Backspace");
  await page.waitForTimeout(100);
  expect(pageErrors).toEqual([]);
  await expect(emoji).toHaveCount(0);
  await expect(editor).toContainText(":wide_emoji");
});

test("上下矢印で入力候補の選択を移動できる", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposerarrows");
  await seedAuth(page, author.token);
  await page.route("**/api/emojis", (route) =>
    route.fulfill({
      json: {
        emojis: [
          { id: "1", aliases: [], name: "first", category: null, host: null, url: "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==" },
          { id: "2", aliases: [], name: "second", category: null, host: null, url: "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==" },
        ],
      },
    })
  );
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.fill(":");
  const options = page.getByRole("option");
  await expect(options).toHaveCount(2);
  await expect(options.nth(0)).toHaveAttribute("aria-selected", "true");
  await editor.press("ArrowDown");
  await expect(options.nth(1)).toHaveAttribute("aria-selected", "true");
  await editor.press("ArrowUp");
  await expect(options.nth(0)).toHaveAttribute("aria-selected", "true");
});
