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

test("投稿直後、リロードなしでも投稿者情報（表示名）がタイムラインに反映される", async ({ page, request }) => {
  // POST /api/notes/create のレスポンス組み立てが display_name/avatar_url を常にNoneで
  // 固定していたため、投稿直後だけプロフィール情報が空になる不具合の回帰テスト
  // （crates/seiran-api/src/handlers/notes/mod.rs の create_regular_post/create_repost）。
  const user = await registerUserViaApi(request, "e2epostname");
  const displayName = `E2E投稿者名${Date.now()}`;
  const patchRes = await request.patch("/api/users/profile", {
    headers: { Authorization: `Bearer ${user.token}` },
    data: { display_name: displayName },
  });
  expect(patchRes.ok(), `プロフィール更新失敗: ${patchRes.status()} ${await patchRes.text()}`).toBeTruthy();

  await seedAuth(page, user.token);
  await page.goto("/");

  const text = `E2E表示名反映テスト ${Date.now()}`;
  await page.getByPlaceholder("いまどうしてる？").fill(text);
  await page.getByRole("button", { name: "投稿", exact: true }).click();

  const note = page.locator("article", { hasText: text });
  await expect(note).toBeVisible({ timeout: 15_000 });
  // リロードせずに表示名が出ていること（バグ時はusernameへフォールバックしてしまう）。
  await expect(note.getByText(displayName)).toBeVisible();
});
