import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("他ユーザーの投稿に返信できる", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ereplyauthor");
  const replier = await registerUserViaApi(request, "e2ereplier");

  const originalText = `元投稿 ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: originalText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const original = await createRes.json();

  await seedAuth(page, replier.token);
  await page.goto(`/notes/${original.id}`);
  await expect(page.getByText(originalText)).toBeVisible({ timeout: 15_000 });

  // 返信ボタンはキャプション文言を持たないため title 属性で特定する。
  await page.getByTitle("返信", { exact: true }).click();
  const replyText = `返信テスト ${Date.now()}`;
  await page.getByPlaceholder("返信を入力").fill(replyText);
  await page.getByRole("button", { name: "投稿", exact: true }).click();

  // モーダルが閉じ、返信フォームが消える = 投稿成功（エラー時はモーダルが開いたまま）。
  await expect(page.getByPlaceholder("返信を入力")).toHaveCount(0, { timeout: 15_000 });
});

// 返信フォームの配送先トグルは、返信先ポストが実際に持つプロトコルのみ表示する
// （持たないプロトコルへ配送すると親と無関係な独立ポストとして誤配信されるため）。
// PostComposer.tsx の fediReplyAllowed/bskyReplyAllowed、バックエンドの
// NoteResponse.replyFediAllowed/replyBskyAllowed の回帰防止。
test.describe("返信フォームの配送先トグル出現制御", () => {
  async function createLocalPost(
    request: import("@playwright/test").APIRequestContext,
    token: string,
    deliverFedi: boolean,
    deliverBsky: boolean,
  ) {
    const text = `トグル確認元投稿 ${Date.now()}-${Math.random()}`;
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${token}` },
      data: { text, deliver_to_fedi: deliverFedi, deliver_to_bsky: deliverBsky, visibility: "public" },
    });
    expect(res.ok(), `create failed: ${res.status()} ${await res.text()}`).toBeTruthy();
    return res.json();
  }

  test("Fedi限定投稿への返信フォームはFediトグルのみ表示される", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2atogglefedi");
    const original = await createLocalPost(request, user.token, true, false);

    await seedAuth(page, user.token);
    await page.goto(`/notes/${original.id}`);
    await page.getByTitle("返信", { exact: true }).click();
    await expect(page.getByPlaceholder("返信を入力")).toBeVisible();

    await expect(page.getByTitle("Fediverseに配送")).toBeVisible();
    await expect(page.getByTitle("Blueskyに配送")).toHaveCount(0);
  });

  test("Bsky限定投稿への返信フォームはBskyトグルのみ表示される", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2atogglebsky");
    const original = await createLocalPost(request, user.token, false, true);

    await seedAuth(page, user.token);
    await page.goto(`/notes/${original.id}`);
    await page.getByTitle("返信", { exact: true }).click();
    await expect(page.getByPlaceholder("返信を入力")).toBeVisible();

    await expect(page.getByTitle("Blueskyに配送")).toBeVisible();
    await expect(page.getByTitle("Fediverseに配送")).toHaveCount(0);
  });

  test("両方に配送した投稿への返信フォームは両方のトグルが表示される", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2atogglebot");
    const original = await createLocalPost(request, user.token, true, true);

    await seedAuth(page, user.token);
    await page.goto(`/notes/${original.id}`);
    await page.getByTitle("返信", { exact: true }).click();
    await expect(page.getByPlaceholder("返信を入力")).toBeVisible();

    await expect(page.getByTitle("Fediverseに配送")).toBeVisible();
    await expect(page.getByTitle("Blueskyに配送")).toBeVisible();
  });
});

// 返信フォームの公開範囲ボタンは、返信先の公開範囲より狭める方向のみ選択可能にする
// （PostComposer.tsx replyVisibilityConstraint）。バックエンドは広げる方向も技術的には
// 許容するが、UIでは意図しない公開範囲の拡大を防ぐため狭める方向のみ提示する。
test.describe("返信フォームの公開範囲ボタン絞り込み", () => {
  async function createLocalPost(
    request: import("@playwright/test").APIRequestContext,
    token: string,
    visibility: "public" | "unlisted" | "followers_only",
    deliverBsky = false,
  ) {
    const text = `公開範囲確認元投稿 ${Date.now()}-${Math.random()}`;
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${token}` },
      data: { text, deliver_to_fedi: true, deliver_to_bsky: deliverBsky, visibility },
    });
    expect(res.ok(), `create failed: ${res.status()} ${await res.text()}`).toBeTruthy();
    return res.json();
  }

  test("パブリック投稿への返信は3段階すべて選べる", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2avispublic");
    const original = await createLocalPost(request, user.token, "public");

    await seedAuth(page, user.token);
    await page.goto(`/notes/${original.id}`);
    await page.getByTitle("返信", { exact: true }).click();
    await expect(page.getByPlaceholder("返信を入力")).toBeVisible();

    await expect(page.getByRole("button", { name: "投稿", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "ひかえめ", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "プライベート", exact: true })).toBeVisible();
  });

  test("ひかえめ投稿への返信はひかえめ・プライベートのみ選べる（パブリックは選べない）", async ({
    page,
    request,
  }) => {
    const user = await registerUserViaApi(request, "e2avisunlist");
    const original = await createLocalPost(request, user.token, "unlisted");

    await seedAuth(page, user.token);
    await page.goto(`/notes/${original.id}`);
    await page.getByTitle("返信", { exact: true }).click();
    await expect(page.getByPlaceholder("返信を入力")).toBeVisible();

    await expect(page.getByRole("button", { name: "投稿", exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "ひかえめ", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "プライベート", exact: true })).toBeVisible();
  });

  test("プライベート投稿への返信はプライベートのみ選べる", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2avisprivate");
    const original = await createLocalPost(request, user.token, "followers_only");

    await seedAuth(page, user.token);
    await page.goto(`/notes/${original.id}`);
    await page.getByTitle("返信", { exact: true }).click();
    await expect(page.getByPlaceholder("返信を入力")).toBeVisible();

    await expect(page.getByRole("button", { name: "投稿", exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "ひかえめ", exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "プライベート", exact: true })).toBeVisible();
  });

  test("Bsky配送される返信ではプライベートボタンが表示されない", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2avisbsky");
    const original = await createLocalPost(request, user.token, "public", true);

    await seedAuth(page, user.token);
    await page.goto(`/notes/${original.id}`);
    await page.getByTitle("返信", { exact: true }).click();
    await expect(page.getByPlaceholder("返信を入力")).toBeVisible();

    // 初期状態は Bsky トグルON（親が両方配送）なのでプライベートは出ない。
    await expect(page.getByRole("button", { name: "プライベート", exact: true })).toHaveCount(0);
    // Bskyトグルをオフにすれば選択肢に戻ってくる。
    await page.getByTitle("Blueskyに配送").click();
    await expect(page.getByRole("button", { name: "プライベート", exact: true })).toBeVisible();
  });
});
