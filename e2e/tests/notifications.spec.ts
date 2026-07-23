import { test, expect } from "@playwright/test";
import { loginViaApi, registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { startStubS3Server, type StubS3Server } from "../fixtures/stub-s3-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

// NotificationsPanel（クイック通知）はHomePageの右ペインにあり、AppShellの
// レスポンシブCSSにより既定のテストビューポート（1280px）幅では非表示になる。
// 右ペインが表示される幅を明示的に指定する。
test.use({ viewport: { width: 1600, height: 900 } });

// global-setup.ts が作成する初期管理者アカウント（role=admin）。
const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e";

// 1x1の最小有効PNG（絵文字アップロードは`prepare_image`で実デコードするためダミーバイト列では通らない）。
const MINIMAL_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC",
  "base64",
);

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

test("カスタム絵文字でリアクションされた通知にカスタム絵文字画像が表示される（#61回帰防止）", async ({ page, request }) => {
  // E2E環境にはS3互換ストレージが無いため、画像アップロードを通すためだけにスタブを起動し、
  // 管理者APIでストレージプロバイダーとして登録する。
  const s3 = await startStubS3Server();
  try {
    const alice = await registerUserViaApi(request, "e2enotifemojia");
    const bob = await registerUserViaApi(request, "e2enotifemojib");
    const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);

    const providerRes = await request.post("/api/admin/storage-providers", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: {
        name: `e2e-stub-${Date.now()}`,
        endpoint: s3.url,
        bucket: "e2e-test",
        access_key: "stub",
        secret_key: "stub",
        public_url: `${s3.url}/e2e-test`,
      },
    });
    expect(providerRes.ok(), `ストレージプロバイダー登録失敗: ${providerRes.status()} ${await providerRes.text()}`).toBeTruthy();

    // カスタム絵文字を1件作成（アップロード→管理者APIで登録）。
    const uploadRes = await request.post("/api/drive/files/create", {
      headers: { Authorization: `Bearer ${bob.token}` },
      multipart: { file: { name: "emoji.png", mimeType: "image/png", buffer: MINIMAL_PNG }, media_type: "emoji" },
    });
    expect(uploadRes.ok(), `絵文字画像アップロード失敗: ${uploadRes.status()} ${await uploadRes.text()}`).toBeTruthy();
    const uploaded = await uploadRes.json();

    const shortcode = `e2eemoji${Date.now().toString(36)}`;
    const createEmojiRes = await request.post("/api/admin/emojis", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: { shortcode, media_file_id: uploaded.id },
    });
    expect(createEmojiRes.ok(), `絵文字登録失敗: ${createEmojiRes.status()} ${await createEmojiRes.text()}`).toBeTruthy();

    const createRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: { text: `カスタム絵文字通知テスト ${Date.now()}` },
    });
    expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
    const created = await createRes.json();

    await seedAuth(page, alice.token);
    await page.goto("/");
    await expect(page.getByText("新しい通知はありません。")).toBeVisible({ timeout: 10_000 });

    const reactRes = await request.post(`/api/notes/${created.id}/reactions`, {
      headers: { Authorization: `Bearer ${bob.token}` },
      data: { content: `:${shortcode}:` },
    });
    expect(reactRes.ok(), `リアクション失敗: ${reactRes.status()} ${await reactRes.text()}`).toBeTruthy();

    const notifText = page.getByText(`${bob.username} がリアクションしました`);
    await expect(notifText).toBeVisible({ timeout: 15_000 });
    // アイコンがフォールバックの絵文字テキストではなく、カスタム絵文字画像として描画されていること
    // （バックエンドの`reactionEmojis`キー形式とフロントの参照キーがずれると画像が出ずフォールバックになる）。
    // `li`単位で絞り込み、絵文字ピッカー等の同じaltを持つ他要素との衝突を避ける。
    const notifItem = page.locator("li", { has: notifText });
    await expect(notifItem.locator(`img[alt="\\:${shortcode}\\:"]`)).toBeVisible({ timeout: 5_000 });
  } finally {
    await s3.close();
  }
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

test("他ユーザーから返信をもらうと返信通知が届き、通知文がユーザーページとリプライ投稿へのリンクになっている", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2enotifreplya");
  const bob = await registerUserViaApi(request, "e2enotifreplyb");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `返信通知テスト ${Date.now()}` },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const created = await createRes.json();

  await seedAuth(page, alice.token);
  await page.goto("/");
  await expect(page.getByText("新しい通知はありません。")).toBeVisible({ timeout: 10_000 });

  const replyRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { text: `返信テスト ${Date.now()}`, reply_to_id: created.id },
  });
  expect(replyRes.ok(), `返信投稿失敗: ${replyRes.status()} ${await replyRes.text()}`).toBeTruthy();
  const reply = await replyRes.json();

  const notifText = page.getByText(`${bob.username} から返信がありました`);
  await expect(notifText).toBeVisible({ timeout: 15_000 });

  // ユーザー名部分はプロフィールページへのリンクになっている。
  const userLink = page.getByRole("link", { name: new RegExp(bob.username) });
  await expect(userLink).toHaveAttribute("href", `/@${bob.username}`);

  // 通知文全体（クリック可能領域）はリプライ投稿へ遷移する。
  await notifText.click();
  await expect(page).toHaveURL(new RegExp(`/notes/${reply.id}$`));
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
