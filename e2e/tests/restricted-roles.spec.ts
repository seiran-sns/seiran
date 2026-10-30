import { test, expect, APIRequestContext } from "@playwright/test";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_URL } from "../ports.ts";
import {
  ADMIN_PASSWORD,
  ADMIN_USERNAME,
  E2eUser,
  getOwnDid,
  loginViaApi,
  patchSiteSettings,
  registerUserViaApi,
  seedAuth,
} from "../fixtures/api-helpers";

async function setRole(request: APIRequestContext, user: E2eUser, role: string) {
  const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  const res = await request.post(`/api/admin/users/${user.userId}/role`, {
    headers: { Authorization: `Bearer ${admin}` },
    data: { role },
  });
  expect(res.ok(), `ロール変更失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
}

async function follow(request: APIRequestContext, follower: E2eUser, target: E2eUser) {
  const res = await request.post("/api/follows/create", {
    headers: { Authorization: `Bearer ${follower.token}` },
    data: { target: target.username },
  });
  expect(res.ok(), `フォロー失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
}

async function createNote(request: APIRequestContext, user: E2eUser, data: Record<string, unknown>) {
  return request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${user.token}` },
    data: { deliver_to_fedi: false, deliver_to_bsky: false, ...data },
  });
}

async function createdId(res: Awaited<ReturnType<typeof createNote>>): Promise<string> {
  expect(res.ok(), `投稿失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()).id as string;
}

async function expectRestricted(res: Awaited<ReturnType<typeof createNote>>) {
  expect(res.status(), await res.text()).toBe(403);
  expect((await res.json()).code).toBe("INTERACTION_RESTRICTED");
}

test("silence はパブリック投稿がひかえめに読み替えられ、投稿フォームのパブリックが押せない", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2esilence");
  await setRole(request, user, "silence");

  const res = await createNote(request, user, { text: `サイレンス投稿 ${Date.now()}`, visibility: "public" });
  expect(res.ok(), `投稿失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  expect((await res.json()).visibility).toBe("unlisted");

  await seedAuth(page, user.token);
  await page.goto("/");
  await page.getByPlaceholder("いまどうしてる？").fill("サイレンス中");
  await expect(page.getByRole("button", { name: "ひかえめ" })).toBeEnabled({ timeout: 15_000 });
  await expect(page.getByRole("button", { name: "投稿", exact: true })).toBeDisabled();
});

test("quarantine はフォロワー以外へ返信・引用・メンション・リアクション・DMできないが、リポストはできる", async ({
  request,
}) => {
  const restricted = await registerUserViaApi(request, "e2equar");
  const stranger = await registerUserViaApi(request, "e2equarstr");
  const follower = await registerUserViaApi(request, "e2equarfol");
  await follow(request, follower, restricted);
  // 制限ユーザーがフォローしているだけの相手は対象外（相手からフォローされていることが条件）
  await follow(request, restricted, stranger);
  await setRole(request, restricted, "quarantine");

  const strangerNote = await createdId(await createNote(request, stranger, { text: `他人の投稿 ${Date.now()}` }));
  const followerNote = await createdId(await createNote(request, follower, { text: `フォロワーの投稿 ${Date.now()}` }));

  await expectRestricted(await createNote(request, restricted, { text: "返信", reply_to_id: strangerNote }));
  await expectRestricted(await createNote(request, restricted, { text: "引用", quote_of_id: strangerNote }));
  await expectRestricted(await createNote(request, restricted, { text: `@${stranger.username} こんにちは` }));
  await expectRestricted(
    await createNote(request, restricted, {
      text: "DM",
      visibility: "direct",
      recipient_actor_ids: [stranger.actorId],
    }),
  );
  await expectRestricted(
    await request.post(`/api/notes/${strangerNote}/reactions`, {
      headers: { Authorization: `Bearer ${restricted.token}` },
      data: { content: "🎉" },
    }),
  );

  const repost = await createNote(request, restricted, { renote_id: strangerNote });
  expect(repost.ok(), `リポスト失敗: ${repost.status()} ${await repost.text()}`).toBeTruthy();

  const reply = await createNote(request, restricted, {
    text: `@${follower.username} 返信`,
    reply_to_id: followerNote,
    visibility: "public",
  });
  expect(reply.ok(), `フォロワーへの返信失敗: ${reply.status()} ${await reply.text()}`).toBeTruthy();
  expect((await reply.json()).visibility).toBe("unlisted");
  const react = await request.post(`/api/notes/${followerNote}/reactions`, {
    headers: { Authorization: `Bearer ${restricted.token}` },
    data: { content: "🎉" },
  });
  expect(react.ok(), `フォロワーへのリアクション失敗: ${react.status()} ${await react.text()}`).toBeTruthy();

  // 調停者（admin / moderator）にはフォロワーでなくても話しかけられる
  const toAdmin = await createNote(request, restricted, { text: `@${ADMIN_USERNAME} 異議があります` });
  expect(toAdmin.ok(), `管理者へのメンション失敗: ${toAdmin.status()} ${await toAdmin.text()}`).toBeTruthy();
});

test("quarantine の新規フォロー数は独自の上限で制限される", async ({ request }) => {
  const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  const restricted = await registerUserViaApi(request, "e2equarfl");
  const targets = [
    await registerUserViaApi(request, "e2equarflt"),
    await registerUserViaApi(request, "e2equarflt"),
    await registerUserViaApi(request, "e2equarflt"),
  ];
  await setRole(request, restricted, "quarantine");
  await patchSiteSettings(request, admin, { follow_rate_limit_max_quarantine: "2" });
  try {
    await follow(request, restricted, targets[0]);
    await follow(request, restricted, targets[1]);
    const third = await request.post("/api/follows/create", {
      headers: { Authorization: `Bearer ${restricted.token}` },
      data: { target: targets[2].username },
    });
    expect(third.status(), await third.text()).toBe(429);
  } finally {
    await patchSiteSettings(request, admin, { follow_rate_limit_max_quarantine: "10" });
  }
});

test("管理画面のレート制限タブはロール×項目のマトリクスで編集できる", async ({ page, request }) => {
  const admin = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  await seedAuth(page, admin);
  await page.goto("/admin/rate-limits");
  await expect(page.getByRole("columnheader", { name: "quarantine" })).toBeVisible({ timeout: 15_000 });

  const cell = page.getByRole("spinbutton", { name: "新規フォロー数 (quarantine)" });
  await expect(cell).toHaveValue("10");
  // 独自の値を持たないロールは入力欄を出さない
  await expect(page.getByRole("spinbutton", { name: "新規フォロー数 (silence)" })).toHaveCount(0);

  try {
    await cell.fill("7");
    await page.getByRole("button", { name: "保存", exact: true }).click();
    await expect(page.getByText("保存しました。")).toBeVisible({ timeout: 10_000 });
    const res = await request.get("/api/admin/site-settings", { headers: { Authorization: `Bearer ${admin}` } });
    expect((await res.json()).follow_rate_limit_max_quarantine).toBe("7");
  } finally {
    await patchSiteSettings(request, admin, { follow_rate_limit_max_quarantine: "10" });
  }
});

// ─── ATP 標準クライアント（Bluesky アプリ等）経由 ─────────────────────────────

interface AtpSession {
  did: string;
  jwt: string;
}

async function atpSession(request: APIRequestContext, user: E2eUser): Promise<AtpSession> {
  const did = await getOwnDid(request, user.token, user.username);
  const res = await request.post(`${BACKEND_URL}/xrpc/com.atproto.server.createSession`, {
    data: { identifier: did, password: user.password },
  });
  expect(res.ok(), `createSession失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  return { did, jwt: (await res.json()).accessJwt as string };
}

function createRecord(request: APIRequestContext, session: AtpSession, collection: string, record: object) {
  return request.post(`${BACKEND_URL}/xrpc/com.atproto.repo.createRecord`, {
    headers: { Authorization: `Bearer ${session.jwt}` },
    data: { repo: session.did, collection, record: { $type: collection, createdAt: new Date().toISOString(), ...record } },
  });
}

async function atpPost(request: APIRequestContext, session: AtpSession, text: string) {
  const res = await createRecord(request, session, "app.bsky.feed.post", { text });
  expect(res.ok(), `createRecord失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()) as { uri: string; cid: string };
}

test("quarantine は Bluesky アプリ経由でもフォロワー以外へ返信・引用・メンション・いいねできない", async ({ request }) => {
  const restricted = await registerUserViaApi(request, "e2equaratp");
  const stranger = await registerUserViaApi(request, "e2equaratps");
  const follower = await registerUserViaApi(request, "e2equaratpf");
  await follow(request, follower, restricted);
  await setRole(request, restricted, "quarantine");

  const me = await atpSession(request, restricted);
  const strangerSession = await atpSession(request, stranger);
  const followerSession = await atpSession(request, follower);
  const strangerPost = await atpPost(request, strangerSession, `他人のATP投稿 ${Date.now()}`);
  const followerPost = await atpPost(request, followerSession, `フォロワーのATP投稿 ${Date.now()}`);
  const replyTo = (ref: { uri: string; cid: string }) => ({ root: ref, parent: ref });

  await expectRestricted(
    await createRecord(request, me, "app.bsky.feed.post", { text: "返信", reply: replyTo(strangerPost) }),
  );
  await expectRestricted(
    await createRecord(request, me, "app.bsky.feed.post", {
      text: "引用",
      embed: { $type: "app.bsky.embed.record", record: strangerPost },
    }),
  );
  // 本文にメンション表記が無くても、mention facet の DID で判定する
  await expectRestricted(
    await createRecord(request, me, "app.bsky.feed.post", {
      text: "hello",
      facets: [
        {
          index: { byteStart: 0, byteEnd: 5 },
          features: [{ $type: "app.bsky.richtext.facet#mention", did: strangerSession.did }],
        },
      ],
    }),
  );
  await expectRestricted(await createRecord(request, me, "app.bsky.feed.like", { subject: strangerPost }));

  const repost = await createRecord(request, me, "app.bsky.feed.repost", { subject: strangerPost });
  expect(repost.ok(), `リポスト失敗: ${repost.status()} ${await repost.text()}`).toBeTruthy();
  const reply = await createRecord(request, me, "app.bsky.feed.post", { text: "返信", reply: replyTo(followerPost) });
  expect(reply.ok(), `フォロワーへの返信失敗: ${reply.status()} ${await reply.text()}`).toBeTruthy();
  const like = await createRecord(request, me, "app.bsky.feed.like", { subject: followerPost });
  expect(like.ok(), `フォロワーへのいいね失敗: ${like.status()} ${await like.text()}`).toBeTruthy();
});

// ─── 他サーバーの相手 ────────────────────────────────────────────────────────

test.describe("quarantine の他サーバー宛ての働きかけ", () => {
  let followerFedi: StubFediServer;
  let strangerFedi: StubFediServer;

  test.beforeEach(async () => {
    followerFedi = await startStubFediServer();
    strangerFedi = await startStubFediServer();
  });

  test.afterEach(async () => {
    await followerFedi.close();
    await strangerFedi.close();
  });

  /** スタブアクターを DB に取り込ませ、その actor_id を返す。全スタブが同じ username を使うので
   * 動的ポート込みの host で絞り込む（`dm.spec.ts` と同じ）。 */
  async function stubActorId(request: APIRequestContext, viewer: E2eUser, fedi: StubFediServer) {
    const fediHost = new URL(fedi.actorUri).host;
    let actorId = "";
    await expect
      .poll(
        async () => {
          const res = await request.get(`/api/actors/search?q=${fediHost}`, {
            headers: { Authorization: `Bearer ${viewer.token}` },
          });
          const found = ((await res.json()) as { actor_id: string; domain: string }[]).find(
            (s) => s.domain === fediHost,
          );
          actorId = found?.actor_id ?? "";
          return actorId;
        },
        { message: `スタブFediアクター(${fediHost})がDB上で見つからない`, timeout: 15_000 },
      )
      .not.toBe("");
    return actorId;
  }

  test("フォロワーでない他サーバーの相手へは DM・メンションできず、フォロワーへは DM できる", async ({ request }) => {
    const restricted = await registerUserViaApi(request, "e2equarfedi");
    await setRole(request, restricted, "quarantine");

    // フォロワー側のスタブ。Follow を受理すると Accept が返る。
    await followerFedi.sendFollow(BACKEND_URL, restricted.username);
    await expect
      .poll(() => followerFedi.receivedActivities().some((a) => a.type === "Accept"), { timeout: 15_000 })
      .toBeTruthy();
    // フォロワーでない側のスタブは、別のユーザーをフォローさせて DB にアクターとして取り込ませる。
    const bystander = await registerUserViaApi(request, "e2equarfedib");
    await strangerFedi.sendFollow(BACKEND_URL, bystander.username);
    await expect
      .poll(() => strangerFedi.receivedActivities().some((a) => a.type === "Accept"), { timeout: 15_000 })
      .toBeTruthy();
    const followerId = await stubActorId(request, restricted, followerFedi);
    const strangerId = await stubActorId(request, restricted, strangerFedi);

    await expectRestricted(
      await createNote(request, restricted, { text: "DM", visibility: "direct", recipient_actor_ids: [strangerId] }),
    );
    const dm = await createNote(request, restricted, {
      text: "DM",
      visibility: "direct",
      recipient_actor_ids: [followerId],
    });
    expect(dm.ok(), `フォロワーへのDM失敗: ${dm.status()} ${await dm.text()}`).toBeTruthy();

    // DB に居ない相手はフォロワーではあり得ないので、メンション表記だけで拒否する。
    await expectRestricted(await createNote(request, restricted, { text: "@nobody@remote.example こんにちは" }));
    await expectRestricted(await createNote(request, restricted, { text: "@nobody.bsky.social こんにちは" }));
  });
});

// ─── 画面上の表示 ──────────────────────────────────────────────────────────

test("quarantine がフォロワー以外へ返信しようとすると、返信フォームに理由が表示される", async ({ page, request }) => {
  const restricted = await registerUserViaApi(request, "e2equarui");
  const stranger = await registerUserViaApi(request, "e2equaruis");
  await setRole(request, restricted, "quarantine");
  const text = `他人の投稿 ${Date.now()}`;
  const noteId = await createdId(await createNote(request, stranger, { text }));

  await seedAuth(page, restricted.token);
  await page.goto(`/notes/${noteId}`);
  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });
  await page.getByTitle("返信", { exact: true }).click();
  await page.getByPlaceholder("返信を入力").fill("返信");
  // パブリック投稿を禁じられているので、返信フォームに 🌐 は出ず 🌙 が既定になる
  await expect(page.getByRole("button", { name: "投稿", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "ひかえめ", exact: true }).click();

  await expect(page.getByText("このアカウントは制限されているため、フォロワー以外への返信")).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByPlaceholder("返信を入力")).toBeVisible();
});
