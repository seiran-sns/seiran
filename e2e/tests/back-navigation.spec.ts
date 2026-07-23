import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("SPA内の戻り先が無い状態で直接ポスト詳細を開き、戻るボタンを押すとホームへ遷移する", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ebacknohist");

  const text = `戻る先なしテスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const note = await createRes.json();

  await seedAuth(page, author.token);
  // SPA内での遷移を経由せず、直接URLを叩く（= SPA内の戻り先が無い状態）。
  await page.goto(`/notes/${note.id}`);
  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });

  await page.getByRole("button", { name: /戻る/ }).click();
  await expect(page).toHaveURL(/\/$/, { timeout: 15_000 });
});

test("SPA内でポスト詳細へ遷移してから戻るボタンを押すと元の画面へ戻る", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ebackwithhist");

  const text = `戻る先ありテスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

  await seedAuth(page, author.token);
  await page.goto("/");
  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });

  await page
    .locator("article", { hasText: text })
    .locator('a[href^="/notes/"]')
    .first()
    .click();
  await expect(page).toHaveURL(/\/notes\//, { timeout: 15_000 });

  await page.getByRole("button", { name: /戻る/ }).click();
  await expect(page).toHaveURL(/\/$/, { timeout: 15_000 });
});
