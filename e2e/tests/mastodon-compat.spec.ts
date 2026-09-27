import crypto from "node:crypto";
import zlib from "node:zlib";
import { test, expect, type APIRequestContext } from "@playwright/test";
import {
  ADMIN_PASSWORD,
  ADMIN_USERNAME,
  loginViaApi,
  registerUserViaApi,
  seedAuth,
} from "../fixtures/api-helpers";
import WebSocket from "ws";
import { startStubS3Server } from "../fixtures/stub-s3-server";
import { BACKEND_PORT } from "../ports.ts";

const OOB = "urn:ietf:wg:oauth:2.0:oob";

/**
 * 呼び出しごとに色の違う 1x1 PNG。seiran はアップロード画像を内容で重複排除するので、
 * 他の spec と同じ画像を使うと先にアップロードした側のストレージプロバイダーの行を共有して
 * しまい（テスト後に無効化される）、後続 spec のアバター URL 等が狂う。
 */
function uniquePng(): Buffer {
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc32 = (buf: Buffer) => {
    let c = 0xffffffff;
    for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type: string, data: Buffer) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([len, body, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(1, 0);
  ihdr.writeUInt32BE(1, 4);
  ihdr[8] = 8; // ビット深度
  ihdr[9] = 2; // RGB
  const rgb = crypto.randomBytes(3);
  const raw = Buffer.concat([Buffer.from([0]), rgb]);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

async function registerApp(request: APIRequestContext, redirectUri: string) {
  // Mastodon クライアントの多くはフォーム形式で登録する。
  const res = await request.post("/api/v1/apps", {
    form: { client_name: "E2E Mastodon Client", redirect_uris: redirectUri, scopes: "read write follow" },
  });
  expect(res.ok(), await res.text()).toBeTruthy();
  return (await res.json()) as { client_id: string; client_secret: string };
}

/** SPA 承認画面を経ずに（承認 API を直接呼んで）Mastodon 用アクセストークンを得る。 */
async function mastodonToken(request: APIRequestContext, userToken: string): Promise<string> {
  const app = await registerApp(request, "e2eclient://callback");
  const authRes = await request.post("/api/oauth/authorize", {
    headers: { Authorization: `Bearer ${userToken}` },
    data: { client_id: app.client_id, redirect_uri: "e2eclient://callback", state: "s1" },
  });
  expect(authRes.ok(), await authRes.text()).toBeTruthy();
  const { redirect_url: redirectUrl, code } = await authRes.json();
  expect(redirectUrl).toBe(`e2eclient://callback?code=${code}&state=s1`);
  const tokenRes = await request.post("/oauth/token", {
    form: {
      grant_type: "authorization_code",
      code,
      client_id: app.client_id,
      client_secret: app.client_secret,
      redirect_uri: "e2eclient://callback",
    },
  });
  expect(tokenRes.ok(), await tokenRes.text()).toBeTruthy();
  return (await tokenRes.json()).access_token;
}

test("Mastodon互換API: 承認画面で許可するとOOBの認可コードが表示され、トークンに交換できる", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2emdoauth");
  const app = await registerApp(request, OOB);
  await seedAuth(page, user.token);

  const query = new URLSearchParams({
    response_type: "code",
    client_id: app.client_id,
    redirect_uri: OOB,
    scope: "read write",
  });
  await page.goto(`/oauth/authorize?${query}`);
  await expect(page).toHaveURL(/\/oauth-connect\?/);
  await expect(page.getByText("E2E Mastodon Client", { exact: false })).toBeVisible();
  await page.getByRole("button").click();

  const codeInput = page.locator("input[readonly]");
  await expect(codeInput).toHaveValue(/^[0-9a-f]{64}$/);
  const code = await codeInput.inputValue();

  const tokenRes = await request.post("/oauth/token", {
    data: {
      grant_type: "authorization_code",
      code,
      client_id: app.client_id,
      client_secret: app.client_secret,
      redirect_uri: OOB,
    },
  });
  expect(tokenRes.ok(), await tokenRes.text()).toBeTruthy();
  const accessToken = (await tokenRes.json()).access_token;

  // 同じコードは二度使えない。
  const reuse = await request.post("/oauth/token", {
    form: {
      grant_type: "authorization_code",
      code,
      client_id: app.client_id,
      client_secret: app.client_secret,
      redirect_uri: OOB,
    },
  });
  expect(reuse.status()).toBe(400);
  expect((await reuse.json()).error).toBe("invalid_grant");

  const me = await request.get("/api/v1/accounts/verify_credentials", {
    headers: { Authorization: `Bearer ${accessToken}` },
  });
  expect(me.ok(), await me.text()).toBeTruthy();
  expect((await me.json()).username).toBe(user.username);

  // ログアウト（revoke）後は使えない。
  const revoke = await request.post("/oauth/revoke", {
    form: { client_id: app.client_id, client_secret: app.client_secret, token: accessToken },
  });
  expect(revoke.ok()).toBeTruthy();
  const after = await request.get("/api/v1/accounts/verify_credentials", {
    headers: { Authorization: `Bearer ${accessToken}` },
  });
  expect(after.status()).toBe(401);
});

test("Mastodon互換API: 登録していないredirect_uriでの認可要求は拒否される", async ({ request }) => {
  const app = await registerApp(request, "e2eclient://callback");
  const res = await request.get(
    `/oauth/authorize?response_type=code&client_id=${app.client_id}&redirect_uri=${encodeURIComponent("evil://steal")}`,
    { maxRedirects: 0 },
  );
  expect(res.status()).toBe(400);
});

test("Mastodon互換API: 投稿・返信・引用・お気に入り・リポストとタイムライン・スレッド・検索", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2emda");
  const bob = await registerUserViaApi(request, "e2emdb");
  const aliceToken = await mastodonToken(request, alice.token);
  const bobToken = await mastodonToken(request, bob.token);
  const asAlice = { Authorization: `Bearer ${aliceToken}` };
  const asBob = { Authorization: `Bearer ${bobToken}` };

  const tag = `mdtag${Date.now().toString(36)}`;
  const createRes = await request.post("/api/v1/statuses", {
    headers: asAlice,
    form: { status: `Mastodon互換の投稿 #${tag}`, visibility: "public" },
  });
  expect(createRes.ok(), await createRes.text()).toBeTruthy();
  const original = await createRes.json();
  expect(original.account.username).toBe(alice.username);
  expect(original.content).toContain(`/tags/${tag}`);
  expect(original.visibility).toBe("public");

  // 返信（JSON 形式）
  const replyRes = await request.post("/api/v1/statuses", {
    headers: asBob,
    data: { status: `@${alice.username} 返信です`, in_reply_to_id: original.id },
  });
  expect(replyRes.ok(), await replyRes.text()).toBeTruthy();
  const reply = await replyRes.json();
  expect(reply.in_reply_to_id).toBe(original.id);
  expect(reply.in_reply_to_account_id).toBe(original.account.id);

  // 引用（Mastodon 4.5 の quoted_status_id）
  const quoteRes = await request.post("/api/v1/statuses", {
    headers: asBob,
    data: { status: "引用します", quoted_status_id: original.id },
  });
  expect(quoteRes.ok(), await quoteRes.text()).toBeTruthy();
  const quote = await quoteRes.json();
  expect(quote.quote.state).toBe("accepted");
  expect(quote.quote.quoted_status.id).toBe(original.id);

  // お気に入り
  const favRes = await request.post(`/api/v1/statuses/${original.id}/favourite`, { headers: asBob });
  expect(favRes.ok(), await favRes.text()).toBeTruthy();
  expect(await favRes.json()).toEqual(expect.objectContaining({ favourited: true, favourites_count: 1 }));
  const favBy = await request.get(`/api/v1/statuses/${original.id}/favourited_by`, { headers: asBob });
  expect((await favBy.json()).map((a: { username: string }) => a.username)).toEqual([bob.username]);

  // リポスト
  const reblogRes = await request.post(`/api/v1/statuses/${original.id}/reblog`, { headers: asBob });
  expect(reblogRes.ok(), await reblogRes.text()).toBeTruthy();
  const reblog = await reblogRes.json();
  expect(reblog.reblog.id).toBe(original.id);
  expect(reblog.reblog.reblogged).toBe(true);

  // Bob のホームタイムラインに自分のリポスト（元投稿を reblog に埋め込み）が出る。
  const home = await request.get("/api/v1/timelines/home?limit=10", { headers: asBob });
  expect(home.ok(), await home.text()).toBeTruthy();
  const homeStatuses = await home.json();
  expect(homeStatuses.some((s: { reblog?: { id: string } }) => s.reblog?.id === original.id)).toBeTruthy();
  expect(home.headers()["link"]).toContain('rel="next"');

  // ハッシュタグタイムライン
  const tagTl = await request.get(`/api/v1/timelines/tag/${tag}`);
  expect(tagTl.ok(), await tagTl.text()).toBeTruthy();
  expect((await tagTl.json()).map((s: { id: string }) => s.id)).toContain(original.id);

  // スレッド
  const ctx = await request.get(`/api/v1/statuses/${reply.id}/context`, { headers: asBob });
  expect(ctx.ok(), await ctx.text()).toBeTruthy();
  expect((await ctx.json()).ancestors.map((s: { id: string }) => s.id)).toEqual([original.id]);

  // アカウント検索・投稿一覧
  const search = await request.get(`/api/v2/search?q=${alice.username}&type=accounts`, { headers: asBob });
  expect(search.ok(), await search.text()).toBeTruthy();
  expect((await search.json()).accounts.map((a: { username: string }) => a.username)).toContain(alice.username);
  const accountStatuses = await request.get(`/api/v1/accounts/${original.account.id}/statuses`);
  expect((await accountStatuses.json()).map((s: { id: string }) => s.id)).toContain(original.id);

  // 取り消し
  const unfav = await request.post(`/api/v1/statuses/${original.id}/unfavourite`, { headers: asBob });
  expect((await unfav.json()).favourited).toBe(false);
  const unreblog = await request.post(`/api/v1/statuses/${original.id}/unreblog`, { headers: asBob });
  expect((await unreblog.json()).reblogged).toBe(false);

  // 削除（下書き復元用の text 付きで返る）
  const del = await request.delete(`/api/v1/statuses/${reply.id}`, { headers: asBob });
  expect(del.ok(), await del.text()).toBeTruthy();
  expect((await del.json()).text).toBe(`@${alice.username} 返信です`);
  const gone = await request.get(`/api/v1/statuses/${reply.id}`);
  expect(gone.status()).toBe(404);
  expect(await gone.json()).toEqual({ error: "RECORD_NOT_FOUND" });
});

test("Mastodon互換API: メディアをアップロードして添付付きで投稿できる", async ({ request }) => {
  const s3 = await startStubS3Server();
  let providerId: string | null = null;
  let adminToken: string | null = null;
  try {
    adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    const providerRes = await request.post("/api/admin/storage-providers", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: {
        name: `e2e-mastodon-media-${Date.now()}`,
        endpoint: s3.url,
        bucket: "e2e-test",
        access_key: "stub",
        secret_key: "stub",
        public_url: `${s3.url}/e2e-test`,
      },
    });
    expect(providerRes.ok(), await providerRes.text()).toBeTruthy();
    providerId = (await providerRes.json()).id;

    const user = await registerUserViaApi(request, "e2emdmedia");
    const token = await mastodonToken(request, user.token);
    const auth = { Authorization: `Bearer ${token}` };

    const upload = await request.post("/api/v2/media", {
      headers: auth,
      multipart: {
        file: { name: "a.png", mimeType: "image/png", buffer: uniquePng() },
        description: "代替テキスト",
      },
    });
    expect(upload.ok(), await upload.text()).toBeTruthy();
    const media = await upload.json();
    expect(media.type).toBe("image");
    expect(media.url).toBeTruthy();

    const post = await request.post("/api/v1/statuses", {
      headers: auth,
      form: { status: "画像つき", "media_ids[]": media.id },
    });
    expect(post.ok(), await post.text()).toBeTruthy();
    const status = await post.json();
    expect(status.media_attachments).toHaveLength(1);
    expect(status.media_attachments[0].type).toBe("image");

    // プロフィール編集（multipart でアバター画像・表示名・項目・承認制を同時に更新）
    const update = await request.patch("/api/v1/accounts/update_credentials", {
      headers: auth,
      multipart: {
        display_name: "マストドン太郎",
        note: "自己紹介です",
        locked: "true",
        "fields_attributes[0][name]": "サイト",
        "fields_attributes[0][value]": "https://example.com",
        avatar: { name: "avatar.png", mimeType: "image/png", buffer: uniquePng() },
      },
    });
    expect(update.ok(), await update.text()).toBeTruthy();
    const me = await update.json();
    expect(me.display_name).toBe("マストドン太郎");
    expect(me.source.note).toBe("自己紹介です");
    expect(me.locked).toBe(true);
    expect(me.fields).toEqual([
      expect.objectContaining({ name: "サイト", value: expect.stringContaining('href="https://example.com"') }),
    ]);
    expect(me.avatar).not.toContain("/api/avatars/");
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

test("Mastodon互換API: ブロック・ミュート・ピン留め・ブックマーク・投票", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2emdc");
  const bob = await registerUserViaApi(request, "e2emdd");
  const asAlice = { Authorization: `Bearer ${await mastodonToken(request, alice.token)}` };
  const asBob = { Authorization: `Bearer ${await mastodonToken(request, bob.token)}` };

  const aliceAccount = await (await request.get(`/api/v1/accounts/lookup?acct=${alice.username}`)).json();

  // ミュート・ブロックと一覧
  const muteRes = await request.post(`/api/v1/accounts/${aliceAccount.id}/mute`, { headers: asBob });
  expect(muteRes.ok(), await muteRes.text()).toBeTruthy();
  expect((await muteRes.json()).muting).toBe(true);
  expect((await (await request.get("/api/v1/mutes", { headers: asBob })).json()).map((a: { id: string }) => a.id)).toEqual([
    aliceAccount.id,
  ]);
  expect((await (await request.post(`/api/v1/accounts/${aliceAccount.id}/unmute`, { headers: asBob })).json()).muting).toBe(false);

  const blockRes = await request.post(`/api/v1/accounts/${aliceAccount.id}/block`, { headers: asBob });
  expect(blockRes.ok(), await blockRes.text()).toBeTruthy();
  expect((await blockRes.json()).blocking).toBe(true);
  expect((await (await request.get("/api/v1/blocks", { headers: asBob })).json()).map((a: { id: string }) => a.id)).toEqual([
    aliceAccount.id,
  ]);
  expect((await (await request.post(`/api/v1/accounts/${aliceAccount.id}/unblock`, { headers: asBob })).json()).blocking).toBe(false);

  // 投票付き投稿へ投票
  const pollPost = await (
    await request.post("/api/v1/statuses", {
      // 配列の繰り返しキー（`poll[options][]=A&poll[options][]=B`）はフォームで送る。
      headers: { ...asAlice, "Content-Type": "application/x-www-form-urlencoded" },
      data: "status=%E3%81%A9%E3%81%A3%E3%81%A1%EF%BC%9F&poll[options][]=A&poll[options][]=B&poll[expires_in]=3600",
    })
  ).json();
  expect(pollPost.poll.options.map((o: { title: string }) => o.title)).toEqual(["A", "B"]);
  const vote = await request.post(`/api/v1/polls/${pollPost.poll.id}/votes`, {
    headers: asBob,
    data: { choices: [1] },
  });
  expect(vote.ok(), await vote.text()).toBeTruthy();
  const voted = await vote.json();
  expect(voted.voted).toBe(true);
  expect(voted.own_votes).toEqual([1]);
  expect(voted.options[1].votes_count).toBe(1);

  // ピン留め（自分の投稿のみ）とプロフィールのピン留め一覧
  const pin = await request.post(`/api/v1/statuses/${pollPost.id}/pin`, { headers: asAlice });
  expect(pin.ok(), await pin.text()).toBeTruthy();
  expect((await pin.json()).pinned).toBe(true);
  const pinned = await request.get(`/api/v1/accounts/${aliceAccount.id}/statuses?pinned=true`);
  expect((await pinned.json()).map((s: { id: string }) => s.id)).toEqual([pollPost.id]);
  expect((await request.post(`/api/v1/statuses/${pollPost.id}/pin`, { headers: asBob })).status()).toBe(403);
  expect((await (await request.post(`/api/v1/statuses/${pollPost.id}/unpin`, { headers: asAlice })).json()).pinned).toBe(false);

  // ブックマーク
  const bm = await request.post(`/api/v1/statuses/${pollPost.id}/bookmark`, { headers: asBob });
  expect(bm.ok(), await bm.text()).toBeTruthy();
  expect((await bm.json()).bookmarked).toBe(true);
  const bookmarks = await request.get("/api/v1/bookmarks", { headers: asBob });
  expect((await bookmarks.json()).map((s: { id: string }) => s.id)).toEqual([pollPost.id]);
  expect((await (await request.post(`/api/v1/statuses/${pollPost.id}/unbookmark`, { headers: asBob })).json()).bookmarked).toBe(false);
  expect(await (await request.get("/api/v1/bookmarks", { headers: asBob })).json()).toEqual([]);
});

test("Mastodon互換API: ストリーミングでホームの新着と通知が届く", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2emde");
  const bob = await registerUserViaApi(request, "e2emdf");
  const aliceToken = await mastodonToken(request, alice.token);
  const bobToken = await mastodonToken(request, bob.token);
  const aliceAccount = await (await request.get(`/api/v1/accounts/lookup?acct=${alice.username}`)).json();
  const bobAccount = await (await request.get(`/api/v1/accounts/lookup?acct=${bob.username}`)).json();

  // Bob が Alice をフォローしておく（Bob のホームに Alice の投稿が流れる）。
  const follow = await request.post(`/api/v1/accounts/${aliceAccount.id}/follow`, {
    headers: { Authorization: `Bearer ${bobToken}` },
  });
  expect((await follow.json()).following).toBe(true);

  const received: { stream: string[]; event: string; payload: string }[] = [];
  const open = (token: string, stream: string) => {
    const ws = new WebSocket(
      `ws://localhost:${BACKEND_PORT}/api/v1/streaming?access_token=${encodeURIComponent(token)}&stream=${stream}`,
    );
    ws.on("message", (data) => {
      try {
        received.push(JSON.parse(data.toString()));
      } catch {
        /* 無視 */
      }
    });
    return new Promise<WebSocket>((resolve, reject) => {
      ws.once("open", () => resolve(ws));
      ws.once("error", reject);
    });
  };
  const bobWs = await open(bobToken, "user");
  const aliceWs = await open(aliceToken, "user");
  try {
    // 接続直後の基準点決め（最新通知の確認）を待つ。
    await new Promise((r) => setTimeout(r, 1000));
    const text = `ストリーミング確認 ${Date.now()}`;
    const post = await (
      await request.post("/api/v1/statuses", {
        headers: { Authorization: `Bearer ${aliceToken}` },
        data: { status: text },
      })
    ).json();

    await expect
      .poll(
        () =>
          received.some(
            (m) => m.event === "update" && m.stream[0] === "user" && JSON.parse(m.payload).id === post.id,
          ),
        { timeout: 15_000 },
      )
      .toBeTruthy();

    // Bob のお気に入りが Alice に通知として届く。
    await request.post(`/api/v1/statuses/${post.id}/favourite`, {
      headers: { Authorization: `Bearer ${bobToken}` },
    });
    await expect
      .poll(
        () =>
          received.some((m) => {
            if (m.event !== "notification") return false;
            const n = JSON.parse(m.payload);
            return n.type === "favourite" && n.account.id === bobAccount.id && n.status?.id === post.id;
          }),
        { timeout: 15_000 },
      )
      .toBeTruthy();
  } finally {
    bobWs.close();
    aliceWs.close();
  }
});
