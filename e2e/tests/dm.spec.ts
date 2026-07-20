import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";

const SEIRAN_BASE_URL = "http://localhost:3000";

test("ローカルユーザー同士のDM送受信・タイムライン除外・既読バッジ", async ({ page, request, browser }) => {
  const alice = await registerUserViaApi(request, "e2dmalice");
  const bob = await registerUserViaApi(request, "e2dmbob");

  const text = `DMテスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text, visibility: "direct", recipient_actor_ids: [bob.actorId] },
  });
  expect(createRes.ok(), `DM作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const created = await createRes.json();

  // bobがまだ開いていない間に2件目を送る。既読カーソルが「最古のメッセージID」ではなく
  // 「最新のメッセージID」を指すことを確認するための下準備（過去の実装ミスの回帰防止）。
  const text2 = `DMテスト2 ${Date.now()}`;
  const createRes2 = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: text2, visibility: "direct", recipient_actor_ids: [bob.actorId], reply_to_id: created.id },
  });
  expect(createRes2.ok(), `DM作成2失敗: ${createRes2.status()} ${await createRes2.text()}`).toBeTruthy();

  // direct投稿はホームタイムラインに一切現れない（フロントは常にexclude_directを付与する）。
  await seedAuth(page, alice.token);
  await page.goto("/");
  await expect(page.getByText(text)).toHaveCount(0, { timeout: 10_000 });

  // bob側: メッセージ履歴に表示され、左ペインのメッセージバッジに未読数が出る。
  const bobContext = await browser.newContext();
  const bobPage = await bobContext.newPage();
  await seedAuth(bobPage, bob.token);
  await bobPage.goto("/");
  await expect(bobPage.getByRole("link", { name: /メッセージ/ })).toBeVisible({ timeout: 10_000 });

  await bobPage.goto(`/messages/${created.id}`);
  // セッション一覧プレビューにも同じ文言が表示されるため、メッセージ履歴側（最初の要素）に絞る。
  await expect(bobPage.getByText(text).first()).toBeVisible({ timeout: 15_000 });
  await expect(bobPage.getByText(text2).first()).toBeVisible({ timeout: 15_000 });

  // 2件（最古・最新）とも受信済みの状態で既読処理すると、バッジが0になる
  // （既読カーソルが最新メッセージIDを指していないと、ここが0にならない回帰バグがあった）。
  await expect
    .poll(async () => {
      const res = await request.get("/api/dm/unread-count", { headers: { Authorization: `Bearer ${bob.token}` } });
      const body = await res.json();
      return body.count;
    }, { timeout: 15_000 })
    .toBe(0);

  await bobContext.close();
});

test("返信送信後に自分のメッセージが重複表示されない", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2dmdupA");
  const bob = await registerUserViaApi(request, "e2dmdupB");

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: "最初のメッセージ", visibility: "direct", recipient_actor_ids: [bob.actorId] },
  });
  expect(createRes.ok()).toBeTruthy();
  const created = await createRes.json();

  await seedAuth(page, alice.token);
  await page.goto(`/messages/${created.id}`);
  await expect(page.getByText("最初のメッセージ").first()).toBeVisible({ timeout: 15_000 });

  const replyText = `重複チェック返信 ${Date.now()}`;
  await page.getByPlaceholder("メッセージを入力…").fill(replyText);
  await page.getByRole("button", { name: "送信" }).click();

  // 送信直後の手動追加とWS再取得（registerDirectMessage）が競合し、同じメッセージが
  // 2つ（右寄せ+左寄せ）描画される回帰バグがあった。
  await expect(page.getByText(replyText)).toHaveCount(1, { timeout: 10_000 });
});

test("通常ポストへの返信としてDMを開始するとスレッド起点が最初のDM投稿になる", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2dmthreadA");
  const bob = await registerUserViaApi(request, "e2dmthreadB");

  const originalText = `通常投稿 ${Date.now()}`;
  const originalRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: originalText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(originalRes.ok()).toBeTruthy();
  const original = await originalRes.json();

  // 通常投稿への返信としてdirectを開始する。
  const dmRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: {
      text: `DM開始 ${Date.now()}`,
      visibility: "direct",
      recipient_actor_ids: [alice.actorId],
      reply_to_id: original.id,
    },
  });
  expect(dmRes.ok(), `DM開始失敗: ${dmRes.status()} ${await dmRes.text()}`).toBeTruthy();
  const dm = await dmRes.json();

  // 同じスレッド内でさらに返信する。
  const replyRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: {
      text: `DM返信 ${Date.now()}`,
      visibility: "direct",
      recipient_actor_ids: [bob.actorId],
      reply_to_id: dm.id,
    },
  });
  expect(replyRes.ok(), `DM返信失敗: ${replyRes.status()} ${await replyRes.text()}`).toBeTruthy();

  // セッション一覧のthreadRootPostIdが「最初のDM投稿」であり、元の通常投稿ではないこと。
  const sessionsRes = await request.get("/api/dm/sessions", { headers: { Authorization: `Bearer ${alice.token}` } });
  expect(sessionsRes.ok()).toBeTruthy();
  const sessions = await sessionsRes.json();
  const session = sessions.find((s: { threadRootPostId: string }) => s.threadRootPostId === dm.id);
  expect(session, "DM開始投稿がスレッド起点になっているセッションが見つからない").toBeTruthy();

  const messagesRes = await request.get(`/api/dm/sessions/${dm.id}/messages`, {
    headers: { Authorization: `Bearer ${alice.token}` },
  });
  expect(messagesRes.ok()).toBeTruthy();
  const messages = await messagesRes.json();
  // 元の通常投稿(original.id)はメッセージ履歴に含まれない（起点はDM投稿自身）。
  expect(messages.some((m: { id: string }) => m.id === original.id)).toBeFalsy();
  expect(messages.length).toBe(2);
});

test.describe("Fedi宛DM配送", () => {
  let fedi: StubFediServer;

  test.beforeEach(async () => {
    fedi = await startStubFediServer();
  });

  test.afterEach(async () => {
    await fedi.close();
  });

  test("宛先アクターのみへ配送され、フォロワー全体には配送されない", async ({ request }) => {
    const alice = await registerUserViaApi(request, "e2dmfedi");
    await fedi.sendFollow(SEIRAN_BASE_URL, alice.username);
    await expect
      .poll(() => fedi.receivedActivities().some((a) => a.type === "Accept"), { timeout: 15_000 })
      .toBeTruthy();

    // 全スタブFediサーバーが同一のusername("e2efedibot")を使うため、他の並行実行中テストの
    // スタブアクターと区別するには、動的ポート込みのhost（例: "127.0.0.1:54321"）で
    // 完全一致するdomainを持つものだけに絞り込む必要がある。
    const fediHost = new URL(fedi.actorUri).host;
    const searchRes = await request.get(`/api/actors/search?q=${fediHost}`, {
      headers: { Authorization: `Bearer ${alice.token}` },
    });
    const suggestions = (await searchRes.json()) as { actor_id: string; domain: string }[];
    const match = suggestions.find((s) => s.domain === fediHost);
    expect(match, "スタブFediアクターがDB上で見つからない").toBeTruthy();
    const fediActorId = match!.actor_id;

    const text = `Fedi DMテスト ${Date.now()}`;
    const createRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: { text, visibility: "direct", recipient_actor_ids: [fediActorId] },
    });
    expect(createRes.ok(), `DM作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

    await expect
      .poll(
        () => fedi.receivedActivities().some((a) => a.type === "Create" && (a.object as any)?.content?.includes(text)),
        { timeout: 15_000 },
      )
      .toBeTruthy();

    const activity = fedi
      .receivedActivities()
      .find((a) => a.type === "Create" && (a.object as any)?.content?.includes(text)) as any;
    // 宛先はスタブアクター本人のURIのみで、フォロワーコレクションではない。
    expect(activity.to).toEqual([fedi.actorUri]);
    expect(activity.cc ?? []).toEqual([]);
  });

  test("Fedi受信のdirect投稿がDMセッションに現れる", async ({ request }) => {
    const alice = await registerUserViaApi(request, "e2dmfedirecv");

    const text = `Fedi DM受信テスト ${Date.now()}`;
    await fedi.sendCreateNote(SEIRAN_BASE_URL, alice.username, text);

    // Inbox受信は非同期処理（Job::InboundActivityProcess）のため反映を待つ。
    await expect
      .poll(
        async () => {
          const res = await request.get("/api/dm/sessions", { headers: { Authorization: `Bearer ${alice.token}` } });
          if (!res.ok()) return false;
          const sessions = (await res.json()) as { lastMessage: { text: string } }[];
          return sessions.some((s) => s.lastMessage.text === text);
        },
        { timeout: 15_000 },
      )
      .toBeTruthy();

    const unreadRes = await request.get("/api/dm/unread-count", { headers: { Authorization: `Bearer ${alice.token}` } });
    expect((await unreadRes.json()).count).toBeGreaterThan(0);
  });
});
