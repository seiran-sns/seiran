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
const ADMIN_PASSWORD = "seiranda-e2e-admin-bootstrap";

// 1x1の最小有効PNG（絵文字アップロードは`prepare_image`で実デコードするためダミーバイト列では通らない）。
const MINIMAL_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC",
  "base64",
);

test("他ユーザーのリアクション通知をホバーすると対象ポストが表示される", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2enotifalice");
  const bob = await registerUserViaApi(request, "e2enotifbob");

  const noteText = `通知テスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: noteText },
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

  const notifText = page.getByText(`${bob.username} がリアクションしました`);
  await expect(notifText).toBeVisible({ timeout: 15_000 });

  await notifText.hover();
  await expect(page.getByRole("link", { name: new RegExp(noteText) })).toBeVisible({ timeout: 5_000 });
});

test("リアクション通知の2行目に、対象ポストの本文（CW があれば警告文）を1行で出す", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2enotiftarget");
  const reactor = await registerUserViaApi(request, "e2enotiftargetr");
  const stamp = Date.now();
  const create = async (data: Record<string, unknown>) => {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${author.token}` },
      data,
    });
    expect(res.ok(), await res.text()).toBeTruthy();
    return (await res.json()) as { id: string };
  };
  const plain = await create({ text: `対象本文1行目 ${stamp}\n\n2行目` });
  const withCw = await create({ text: `隠れた本文 ${stamp}`, content_warning: `警告文 ${stamp}` });
  for (const note of [plain, withCw]) {
    const res = await request.post(`/api/notes/${note.id}/reactions`, {
      headers: { Authorization: `Bearer ${reactor.token}` },
      data: { content: "👍" },
    });
    expect(res.ok(), await res.text()).toBeTruthy();
  }

  await seedAuth(page, author.token);
  await page.goto("/notifications");
  // 同じ人からの2件は1つに重ねて出るので、li ではなく通知カード1枚ずつを数える。
  const items = page
    .locator("main [data-notification-id]")
    .filter({ hasText: `${reactor.username} がリアクションしました` });
  await expect(items).toHaveCount(2, { timeout: 15_000 });
  // 改行は半角スペースでつなぐ。
  await expect(items.filter({ hasText: `対象本文1行目 ${stamp} 2行目` })).toHaveCount(1);
  // CW があれば本文ではなく警告文。
  await expect(items.filter({ hasText: `警告文 ${stamp}` })).toHaveCount(1);
  await expect(items.filter({ hasText: `隠れた本文 ${stamp}` })).toHaveCount(0);
});

test("同じポストへの連続したリアクション通知は重ねて出し、マウスが乗ったカードを前面に出す", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2enotifstack");
  const res = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: `重ね表示テスト ${Date.now()}` },
  });
  expect(res.ok(), await res.text()).toBeTruthy();
  const note = (await res.json()) as { id: string };
  for (const name of ["e2enotifstacka", "e2enotifstackb", "e2enotifstackc"]) {
    const reactor = await registerUserViaApi(request, name);
    const r = await request.post(`/api/notes/${note.id}/reactions`, {
      headers: { Authorization: `Bearer ${reactor.token}` },
      data: { content: "👍" },
    });
    expect(r.ok(), await r.text()).toBeTruthy();
  }

  await seedAuth(page, author.token);
  await page.goto("/notifications");
  const stack = page.locator('main li[class*="stack"]');
  await expect(stack).toHaveCount(1, { timeout: 15_000 });
  const cards = stack.locator(':scope > div');
  await expect(cards).toHaveCount(3);
  // 前面のカード（DOM の最後）の位置。初期は一番新しいカード（ずらし 0）。
  const frontTop = () => cards.last().evaluate((el) => (el as HTMLElement).style.top);
  await page.mouse.move(0, 0);
  await expect.poll(frontTop).toBe("0px");

  // 一番下の帯（一番古いカードの下端）にマウスを乗せると、そのカードが前面に出る。
  const box = await stack.boundingBox();
  if (!box) throw new Error("stack not laid out");
  await page.mouse.move(box.x + box.width / 2, box.y + box.height - 2);
  await expect.poll(frontTop).toBe("16px");
});

test("他ユーザーによるリポストと引用が通知される（#198）", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2enotifshareauthor");
  const sharer = await registerUserViaApi(request, "e2enotifsharer");
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: `リポスト・引用通知元 ${Date.now()}` },
  });
  expect(createRes.ok(), await createRes.text()).toBeTruthy();
  const original = (await createRes.json()) as { id: string };

  const repostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${sharer.token}` },
    data: { renote_id: original.id, deliver_to_fedi: false, deliver_to_bsky: false },
  });
  expect(repostRes.ok(), await repostRes.text()).toBeTruthy();
  const quoteRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${sharer.token}` },
    data: {
      text: "引用通知テスト",
      quote_of_id: original.id,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
    },
  });
  expect(quoteRes.ok(), await quoteRes.text()).toBeTruthy();

  const notifsRes = await request.post("/api/i/notifications", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { limit: 20, markAsRead: false },
  });
  expect(notifsRes.ok(), await notifsRes.text()).toBeTruthy();
  const notifs = (await notifsRes.json()) as Array<{ type: string; user?: { username: string } }>;
  // "renote" はMisskey本家API仕様での呼称（seiran内部では「リポスト」と呼ぶ種別と同じもの）。
  expect(notifs.some((n) => n.type === "renote" && n.user?.username === sharer.username)).toBeTruthy();
  expect(notifs.some((n) => n.type === "quote" && n.user?.username === sharer.username)).toBeTruthy();
});

test("アンリポスト（取り消し）後もリポスト通知をホバーすると元ポストが表示される", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2enotifundoauthor");
  const sharer = await registerUserViaApi(request, "e2enotifundosharer");

  const noteText = `アンリポスト通知テスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: noteText },
  });
  expect(createRes.ok(), await createRes.text()).toBeTruthy();
  const original = (await createRes.json()) as { id: string };

  const repostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${sharer.token}` },
    data: { renote_id: original.id, deliver_to_fedi: false, deliver_to_bsky: false },
  });
  expect(repostRes.ok(), await repostRes.text()).toBeTruthy();

  // リポストを取り消す（リポストラッパー投稿が論理削除される）。通知自体は残るが、
  // 通知一覧APIが取り消し済みラッパー経由で元ポスト情報（note.renote）を正しく
  // 復元できることを検証する。
  const undoRes = await request.delete(`/api/notes/${original.id}/repost`, {
    headers: { Authorization: `Bearer ${sharer.token}` },
  });
  expect(undoRes.ok(), await undoRes.text()).toBeTruthy();

  await seedAuth(page, author.token);
  await page.goto("/");

  const notifText = page.getByText(`${sharer.username} がリポストしました`);
  await expect(notifText).toBeVisible({ timeout: 15_000 });

  await notifText.hover();
  await expect(page.getByRole("link", { name: new RegExp(noteText) })).toBeVisible({ timeout: 5_000 });
});

test("カスタム絵文字でリアクションされた通知にカスタム絵文字画像が表示される（#61回帰防止）", async ({ page, request }) => {
  // E2E環境にはS3互換ストレージが無いため、画像アップロードを通すためだけにスタブを起動し、
  // 管理者APIでストレージプロバイダーとして登録する。
  const s3 = await startStubS3Server();
  let providerId: string | null = null;
  let adminToken: string | null = null;
  try {
    const alice = await registerUserViaApi(request, "e2enotifemojia");
    const bob = await registerUserViaApi(request, "e2enotifemojib");
    adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);

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
    providerId = (await providerRes.json()).id;

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
    // アイコンのalt文字列（`n.reaction`由来）は本家Misskey準拠でローカルは `@.` サフィックス付き。
    const notifItem = page.locator("li", { has: notifText });
    await expect(notifItem.locator(`img[alt="\\:${shortcode}@.\\:"]`)).toBeVisible({ timeout: 5_000 });
  } finally {
    if (providerId && adminToken) {
      await request.patch(`/api/admin/storage-providers/${providerId}`, {
        headers: { Authorization: `Bearer ${adminToken}` },
        data: { is_active: false },
      });
    }
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

test("スマホ表示の中央下フローティングボタンから通知画面へ遷移できる（#75）", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2enotifmobile");
  await seedAuth(page, user.token);

  await page.setViewportSize({ width: 375, height: 812 });
  await page.goto("/");

  await page.getByRole("link", { name: "通知", exact: true }).click();
  await expect(page).toHaveURL(/\/notifications$/);
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

  // 通知文全体（クリック可能領域）はリプライ投稿へ遷移する。ユーザー名リンクは
  // クリック伝播を止める実装（NotificationsPanel.tsx）で、userLinkとサフィックス
  // テキストは同じ`.text`span内の兄弟ノードのため、`getByText`でサフィックス文字列
  // だけを指定してもPlaywrightはその文字列を含む最小の要素（結局この`.text`span
  // 全体）に解決し、クリックはその要素のバウンディングボックス中心に対して行われる。
  // usernameが長いとその中心がuserLink側に寄ってしまいクリックを奪われる
  // （trace調査で実際に`.text`spanへ解決されクリック座標がuserLink上に乗っていたことを確認済み）。
  // username長に依存しない安全な標的として、`.text`の外側にあるリプライ種別アイコン
  // （常にstopPropagationを持たない）をクリックする。ノートカードの返信ボタンも同じ
  // 絵文字（💬）を使うため、通知の行（li）にスコープしてから探す。
  const notifItem = page.locator("li", { hasText: `${bob.username} から返信がありました` });
  await notifItem.getByAltText("💬").click();
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

test("返信文中に返信先へ@メンションしても通知はreplyのみでmentionは重複作成されない", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2enotifreplymenta");
  const bob = await registerUserViaApi(request, "e2enotifreplymentb");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `返信+メンション重複テスト元投稿 ${Date.now()}` },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const created = await createRes.json();

  const replyRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { text: `@${alice.username} 返信+メンション重複テスト ${Date.now()}`, reply_to_id: created.id },
  });
  expect(replyRes.ok(), `返信投稿失敗: ${replyRes.status()} ${await replyRes.text()}`).toBeTruthy();
  const reply = await replyRes.json();

  const notifRes = await request.post("/api/i/notifications", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { limit: 10 },
  });
  expect(notifRes.ok()).toBeTruthy();
  const notifs = (await notifRes.json()) as { type: string; note?: { id: string } }[];
  const forReply = notifs.filter((n) => n.note?.id === String(reply.id));
  expect(forReply.map((n) => n.type)).toEqual(["reply"]);
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
