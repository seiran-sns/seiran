import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// NotificationsPanel（クイック通知）はHomePageの右ペインにあり、AppShellの
// レスポンシブCSSにより既定のテストビューポート（1280px）幅では非表示になる。
// 右ペインが表示される幅を明示的に指定する。
test.use({ viewport: { width: 1600, height: 900 } });

test("他ユーザーのリアクションが通知として届き一覧に表示される", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2enotifalice");
  const bob = await registerUserViaApi(request, "e2enotifbob");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `通知テスト ${Date.now()}` },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const created = await createRes.json();

  await seedAuth(page, alice.token);
  await page.goto("/");
  // クイック通知タブ（右ペイン既定タブ）が空の状態から始まることを確認してから、
  // bobのリアクションがWS経由でリアルタイムに一覧へ反映されることを検証する。
  await expect(page.getByText("新しい通知はありません。")).toBeVisible({ timeout: 10_000 });

  const reactRes = await request.post(`/api/notes/${created.id}/reactions`, {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { content: "🎉" },
  });
  expect(reactRes.ok(), `リアクション失敗: ${reactRes.status()} ${await reactRes.text()}`).toBeTruthy();

  await expect(page.getByText(`${bob.username} がリアクションしました`)).toBeVisible({ timeout: 15_000 });
});
