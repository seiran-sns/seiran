import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

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

test("通知ページ(/notifications)の中央ペインにクイック通知パネルが表示される", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2enotifpagea");
  const bob = await registerUserViaApi(request, "e2enotifpageb");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `通知ページテスト ${Date.now()}` },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const created = await createRes.json();

  const reactRes = await request.post(`/api/notes/${created.id}/reactions`, {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { content: "🎉" },
  });
  expect(reactRes.ok(), `リアクション失敗: ${reactRes.status()} ${await reactRes.text()}`).toBeTruthy();

  // ホーム右ペインのクイック通知タブではなく、専用ページ(/notifications)へ直接遷移した
  // 場合でも、既存にたまっている通知がREST経由で中央ペインに表示されることを検証する。
  await seedAuth(page, alice.token);
  await page.goto("/notifications");
  await expect(page.getByText(`${bob.username} がリアクションしました`)).toBeVisible({ timeout: 10_000 });
});

test("ローカル投稿で@メンションすると相手にメンション通知が届く", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2enotifmentiona");
  const bob = await registerUserViaApi(request, "e2enotifmentionb");

  await seedAuth(page, bob.token);
  await page.goto("/");
  await expect(page.getByText("新しい通知はありません。")).toBeVisible({ timeout: 10_000 });

  const text = `@${bob.username} メンション通知テスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

  await expect(page.getByText(`${alice.username} からメンションされました`)).toBeVisible({ timeout: 15_000 });
});

test("自分自身への@メンションは通知されない", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2enotifselfmention");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `@${alice.username} 自己メンション ${Date.now()}` },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

  const notifRes = await request.post("/api/i/notifications", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { limit: 5 },
  });
  expect(notifRes.ok()).toBeTruthy();
  const notifs = (await notifRes.json()) as { type: string }[];
  expect(notifs.some((n) => n.type === "mention")).toBeFalsy();
});

test.describe("Fedi(AP)からのメンション通知", () => {
  let fedi: StubFediServer;

  test.beforeEach(async () => {
    fedi = await startStubFediServer();
  });

  test.afterEach(async () => {
    await fedi.close();
  });

  test("Fediから届いたMentionタグ付き投稿がメンション通知として届く", async ({ request }) => {
    const bob = await registerUserViaApi(request, "e2enotifapmention");

    await fedi.sendCreateNote(SEIRAN_BASE_URL, bob.username, `Fediメンションテスト ${Date.now()}`, {
      mentionTargetUsername: bob.username,
    });

    // Inbox受信は非同期処理（Job::InboundActivityProcess）のため反映を待つ。
    await expect
      .poll(
        async () => {
          const res = await request.post("/api/i/notifications", {
            headers: { Authorization: `Bearer ${bob.token}` },
            data: { limit: 5 },
          });
          if (!res.ok()) return false;
          const notifs = (await res.json()) as { type: string }[];
          return notifs.some((n) => n.type === "mention");
        },
        { timeout: 15_000 },
      )
      .toBeTruthy();
  });
});
