import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("ホーム画面から他画面へ遷移して戻ると、選択タブとスクロール位置が保持される", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2ehomestate");

  // スクロールが発生するよう十分な件数の投稿を作る。
  for (let i = 0; i < 25; i++) {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { text: `ホーム状態保持テスト投稿 ${i} ${Date.now()}` },
    });
    expect(res.ok(), `投稿作成失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  }

  await seedAuth(page, user.token);
  await page.goto("/");

  // ローカルタブへ切り替える。
  await page.getByRole("button", { name: "ローカル" }).click();
  await expect(page.getByRole("button", { name: "ローカル" })).toHaveClass(/feedTabActive/);

  // 投稿一覧が描画されるのを待ってからスクロールする。
  await expect(page.locator("article").first()).toBeVisible({ timeout: 15_000 });
  await page.evaluate(() => window.scrollTo(0, 400));
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(200);
  const scrollYBefore = await page.evaluate(() => window.scrollY);

  // 別画面（通知）へ遷移してからホームへ戻る（SPA内遷移、フルリロードなし）。
  await page.getByRole("link", { name: /通知/ }).click();
  await expect(page).toHaveURL(/\/notifications$/);
  await page.getByRole("link", { name: /ホーム/ }).click();
  await expect(page).toHaveURL(/\/$/);

  // タブ選択がローカルのまま保持されている。
  await expect(page.getByRole("button", { name: "ローカル" })).toHaveClass(/feedTabActive/, { timeout: 10_000 });

  // スクロール位置も復元されている（キャッシュ復元後にrAFで反映されるため多少の遅延を許容）。
  await expect.poll(() => page.evaluate(() => window.scrollY), { timeout: 10_000 }).toBeGreaterThan(200);
  const scrollYAfter = await page.evaluate(() => window.scrollY);
  expect(Math.abs(scrollYAfter - scrollYBefore)).toBeLessThan(50);
});
