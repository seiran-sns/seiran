import { test, expect } from "@playwright/test";
import WebSocket from "ws";
import { loginViaApi, registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_PORT, BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

// global-setup.ts が作成する初期管理者アカウント（role=admin）。
const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e-admin-bootstrap";

// 管理者はモデレーションのため、公開範囲外のローカル投稿を詳細画面で閲覧できる。ただし本来
// 見えない投稿なので、カードはグレー表示・操作不可で、操作APIは NOTE_NOT_FOUND を返す。
test("管理者は公開範囲外のローカル投稿をグレー表示・操作不可で閲覧できる", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2eadmovr");
  const created = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: {
      text: `管理者閲覧確認 ${Date.now()}`,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
      visibility: "followers_only",
    },
  });
  expect(created.ok(), `create failed: ${created.status()} ${await created.text()}`).toBeTruthy();
  const noteId = ((await created.json()) as { id: string }).id;

  const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  const auth = { Authorization: `Bearer ${adminToken}` };

  const shown = await request.get(`/api/notes/${noteId}`, { headers: auth });
  expect(shown.status()).toBe(200);
  expect((await shown.json()).visibleByAdminOverride).toBe(true);

  const react = await request.post(`/api/notes/${noteId}/reactions`, {
    headers: auth,
    data: { content: "👍" },
  });
  expect(react.status()).toBe(404);
  expect((await react.json()).code).toBe("NOTE_NOT_FOUND");

  // 投稿者本人は通常どおり閲覧でき、フラグは付かない。
  const own = await request.get(`/api/notes/${noteId}`, {
    headers: { Authorization: `Bearer ${author.token}` },
  });
  expect((await own.json()).visibleByAdminOverride).toBeUndefined();

  // プロフィールの投稿一覧でも同じフラグが付く（公開投稿には付かない）。
  const publicRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: `管理者閲覧確認(公開) ${Date.now()}`, deliver_to_fedi: false, deliver_to_bsky: false },
  });
  expect(publicRes.ok(), await publicRes.text()).toBeTruthy();
  const publicId = ((await publicRes.json()) as { id: string }).id;
  const profileRes = await request.get(`/api/users/profile?q=${encodeURIComponent(author.username)}`, {
    headers: auth,
  });
  expect(profileRes.ok(), await profileRes.text()).toBeTruthy();
  const profile = (await profileRes.json()) as {
    recent_posts: Array<{ id: string; visibleByAdminOverride?: boolean }>;
  };
  expect(profile.recent_posts.find((n) => n.id === String(noteId))?.visibleByAdminOverride).toBe(true);
  const publicEntry = profile.recent_posts.find((n) => n.id === String(publicId));
  expect(publicEntry).toBeDefined();
  expect(publicEntry?.visibleByAdminOverride).toBeUndefined();

  await seedAuth(page, adminToken);
  await page.goto(`/notes/${noteId}`);
  await expect(page.getByTitle("返信", { exact: true })).toBeDisabled({ timeout: 10_000 });
  await expect(page.getByTitle("リアクションを付ける", { exact: true })).toBeDisabled();
  await expect(page.locator("article[class*='adminOverride']")).toHaveCount(1);
});

test.describe("リモートの非公開投稿", () => {
  let fedi: StubFediServer;

  test.beforeEach(async () => {
    fedi = await startStubFediServer();
  });

  test.afterEach(async () => {
    await fedi.close();
  });

  // 管理者特権による閲覧はローカル投稿だけが対象。リモートの非公開投稿は、自サーバーの
  // 宛先ユーザーに届いたものにすぎずモデレーションの対象外なので、管理者にも見えない。
  test("管理者でも閲覧できず NOTE_NOT_FOUND になる", async ({ request }) => {
    const recipient = await registerUserViaApi(request, "e2eadmovrr");
    const text = `リモート非公開 ${Date.now()}`;
    await fedi.sendCreateNote(SEIRAN_BASE_URL, recipient.username, text);

    let noteId = "";
    await expect
      .poll(
        async () => {
          const res = await request.get("/api/dm/sessions", {
            headers: { Authorization: `Bearer ${recipient.token}` },
          });
          if (!res.ok()) return false;
          const sessions = (await res.json()) as { lastMessage: { id: string; text: string } }[];
          noteId = sessions.find((s) => s.lastMessage.text === text)?.lastMessage.id ?? "";
          return noteId !== "";
        },
        { timeout: 15_000 },
      )
      .toBeTruthy();

    const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    const res = await request.get(`/api/notes/${noteId}`, {
      headers: { Authorization: `Bearer ${adminToken}` },
    });
    expect(res.status()).toBe(404);
    expect((await res.json()).code).toBe("NOTE_NOT_FOUND");
  });
});

async function createPost(
  request: import("@playwright/test").APIRequestContext,
  token: string,
  data: Record<string, unknown>,
): Promise<string> {
  const res = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${token}` },
    data: { deliver_to_fedi: false, deliver_to_bsky: false, ...data },
  });
  expect(res.ok(), `create failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  return String(((await res.json()) as { id: string }).id);
}

// 管理者のモデレーション表示: ローカルTLにひかえめ・プライベートも混ぜる（DMは出さない）。
// 管理者自身から本来見えないプライベートだけがグレー表示（visibleByAdminOverride）になる。
test.describe("ローカルTLのモデレーション表示", () => {
  test("APIはひかえめ・プライベートを含めDMを含めず、管理者以外は拒否する", async ({ request }) => {
    const author = await registerUserViaApi(request, "e2eadmltl");
    const peer = await registerUserViaApi(request, "e2eadmltlp");
    const stamp = Date.now();
    const followersId = await createPost(request, author.token, {
      text: `LTLモデレーション(プライベート) ${stamp}`,
      visibility: "followers_only",
    });
    const unlistedId = await createPost(request, author.token, {
      text: `LTLモデレーション(ひかえめ) ${stamp}`,
      visibility: "unlisted",
    });
    const dmId = await createPost(request, author.token, {
      text: `LTLモデレーション(DM) ${stamp}`,
      visibility: "direct",
      recipient_actor_ids: [peer.actorId],
    });

    const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    const res = await request.get("/api/notes/local-timeline?moderation=true&exclude_direct=true&limit=100", {
      headers: { Authorization: `Bearer ${adminToken}` },
    });
    expect(res.ok(), await res.text()).toBeTruthy();
    const notes = (await res.json()) as Array<{ id: string; visibleByAdminOverride?: boolean }>;
    const byId = new Map(notes.map((n) => [String(n.id), n]));
    expect(byId.get(followersId)?.visibleByAdminOverride).toBe(true);
    expect(byId.has(unlistedId)).toBe(true);
    expect(byId.get(unlistedId)?.visibleByAdminOverride).toBeUndefined();
    expect(byId.has(dmId)).toBe(false);

    // 通常のローカルTLには出ない。
    const normal = await request.get("/api/notes/local-timeline?exclude_direct=true&limit=100", {
      headers: { Authorization: `Bearer ${adminToken}` },
    });
    const normalIds = ((await normal.json()) as Array<{ id: string }>).map((n) => String(n.id));
    expect(normalIds).not.toContain(followersId);

    const denied = await request.get("/api/notes/local-timeline?moderation=true", {
      headers: { Authorization: `Bearer ${peer.token}` },
    });
    expect(denied.status()).toBe(403);
    expect((await denied.json()).code).toBe("ADMIN_REQUIRED");
  });

  test("プロフィール画面の投稿一覧APIでも管理者にはプライベートがフラグ付きで出る", async ({ request }) => {
    const author = await registerUserViaApi(request, "e2eadmprof");
    const followersId = await createPost(request, author.token, {
      text: `プロフィール一覧確認 ${Date.now()}`,
      visibility: "followers_only",
    });
    const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    const res = await request.get(
      `/api/users/posts?actor_id=${author.actorId}&limit=20&exclude_direct=true&includeReactions=true`,
      { headers: { Authorization: `Bearer ${adminToken}` } },
    );
    expect(res.ok(), await res.text()).toBeTruthy();
    const items = (await res.json()) as Array<{ id?: string; note?: { id: string }; visibleByAdminOverride?: boolean }>;
    const raw = JSON.stringify(items);
    expect(raw).toContain(followersId);
    expect(raw).toContain('"visibleByAdminOverride":true');
  });

  test("WebSocketのモデレーションチャンネルは管理者だけが購読でき、プライベートがフラグ付きで届く", async ({
    request,
  }) => {
    const author = await registerUserViaApi(request, "e2eadmws");
    const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);

    const ws = new WebSocket(`ws://localhost:${BACKEND_PORT}/api/streaming?token=${encodeURIComponent(adminToken)}`);
    const received: Array<{ type: string; body: { id: string; body?: { text?: string; visibleByAdminOverride?: boolean } } }> =
      [];
    ws.addEventListener("message", (ev) => received.push(JSON.parse(ev.data as string)));
    const denied = new WebSocket(`ws://localhost:${BACKEND_PORT}/api/streaming?token=${encodeURIComponent(author.token)}`);
    const deniedReceived: Array<{ type: string; body: { id: string } }> = [];
    denied.addEventListener("message", (ev) => deniedReceived.push(JSON.parse(ev.data as string)));
    try {
      await Promise.all(
        [ws, denied].map(
          (s) => new Promise<void>((resolve, reject) => {
            s.once("open", () => resolve());
            s.once("error", () => reject(new Error("WebSocket接続に失敗しました")));
          }),
        ),
      );
      ws.send(JSON.stringify({ type: "connect", body: { channel: "localTimelineModeration", id: "mod", params: {} } }));
      denied.send(JSON.stringify({ type: "connect", body: { channel: "localTimelineModeration", id: "mod", params: {} } }));
      await expect
        .poll(() => deniedReceived.some((m) => m.type === "error" && m.body?.id === "mod"), { timeout: 10_000 })
        .toBeTruthy();
      // 購読が登録されるのを待ってから投稿する。
      await new Promise((r) => setTimeout(r, 500));

      const text = `WSモデレーション ${Date.now()}`;
      await createPost(request, author.token, { text, visibility: "followers_only" });
      await expect
        .poll(
          () =>
            received.find((m) => m.type === "channel" && m.body?.id === "mod" && m.body?.body?.text === text)?.body?.body
              ?.visibleByAdminOverride,
          { timeout: 15_000 },
        )
        .toBe(true);
    } finally {
      ws.close();
      denied.close();
    }
  });

  test("画面のローカルTLで切り替えるとプライベートがグレー表示で並ぶ", async ({ page, request }) => {
    const author = await registerUserViaApi(request, "e2eadmui");
    const text = `画面モデレーション ${Date.now()}`;
    await createPost(request, author.token, { text, visibility: "followers_only" });
    const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    await seedAuth(page, adminToken);

    await page.goto("/");
    await page.getByRole("button", { name: "ローカル", exact: true }).click();
    const toggle = page.getByLabel("モデレーション表示（ひかえめ・プライベートの投稿も表示する）");
    await expect(toggle).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText(text)).toHaveCount(0);
    await toggle.check();
    const card = page.locator("article[class*='adminOverride']", { hasText: text });
    await expect(card).toBeVisible({ timeout: 10_000 });
    await expect(card.getByTitle("返信", { exact: true })).toBeDisabled();
    // 戻すと消える。
    await toggle.uncheck();
    await expect(page.getByText(text)).toHaveCount(0, { timeout: 10_000 });
  });
});
