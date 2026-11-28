import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { APPVIEW_STUB_PORT } from "../ports.ts";

const APPVIEW_CONTROL_URL = `http://127.0.0.1:${APPVIEW_STUB_PORT}`;

test("AppViewの未知投稿を保存してuntil/sinceページングへ統合する（#146）", async ({
  request,
}) => {
  const viewer = await registerUserViaApi(request, "e2esearchappview");
  const unique = `appview-search-${Date.now()}`;
  const createdAt = new Date(Date.now() - 60_000).toISOString();
  const did = `did:plc:e2esearch${Date.now().toString(36)}`;
  const appviewText = `${unique} AppView未知投稿`;

  const configure = await request.post(`${APPVIEW_CONTROL_URL}/__control__/search`, {
    data: {
      posts: [
        {
          uri: `at://${did}/app.bsky.feed.post/test`,
          cid: `bafy${Date.now().toString(36)}`,
          text: appviewText,
          createdAt,
          author: {
            did,
            handle: `search-${Date.now().toString(36)}.bsky.social`,
            displayName: "AppView Search User",
            avatar: "https://example.com/appview-avatar.png",
          },
        },
      ],
    },
  });
  expect(configure.ok(), await configure.text()).toBeTruthy();

  const initial = await request.get(
    `/api/notes/search?q=${encodeURIComponent(unique)}&limit=1`,
    { headers: { Authorization: `Bearer ${viewer.token}` } },
  );
  expect(initial.ok(), await initial.text()).toBeTruthy();
  const initialBody = (await initial.json()) as {
    notes: { text: string; user: { avatarUrl?: string } }[];
  };
  expect(initialBody.notes.map((note) => note.text)).toContain(appviewText);
  expect(initialBody.notes.find((note) => note.text === appviewText)?.user.avatarUrl).toBe(
    "https://example.com/appview-avatar.png",
  );

  const misskey = await request.post("/api/notes/search", {
    headers: { Authorization: `Bearer ${viewer.token}` },
    data: { query: unique, limit: 10 },
  });
  expect(misskey.ok(), await misskey.text()).toBeTruthy();
  expect(
    ((await misskey.json()) as { text: string }[]).map((note) => note.text),
  ).toContain(appviewText);

  const anchor = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${viewer.token}` },
    data: {
      text: `${unique} until anchor`,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
      visibility: "public",
    },
  });
  expect(anchor.ok(), await anchor.text()).toBeTruthy();
  const anchorNote = (await anchor.json()) as { id: string };

  const until = await request.get(
    `/api/notes/search?q=${encodeURIComponent(unique)}&limit=1&until_id=${anchorNote.id}`,
    { headers: { Authorization: `Bearer ${viewer.token}` } },
  );
  expect(until.ok(), await until.text()).toBeTruthy();

  const requestsRes = await request.get(
    `${APPVIEW_CONTROL_URL}/__control__/search/requests`,
  );
  const appviewRequests = (await requestsRes.json()) as {
    limit: string | null;
    until: string | null;
  }[];
  expect(appviewRequests.at(-1)).toEqual(
    expect.objectContaining({ limit: "1", until: expect.any(String) }),
  );

  const clear = await request.post(`${APPVIEW_CONTROL_URL}/__control__/search`, {
    data: { posts: [] },
  });
  expect(clear.ok(), await clear.text()).toBeTruthy();

  const since = await request.get(
    `/api/notes/search?q=${encodeURIComponent(unique)}&limit=10&since_id=1`,
    { headers: { Authorization: `Bearer ${viewer.token}` } },
  );
  expect(since.ok(), await since.text()).toBeTruthy();
  expect((await since.json()).notes.map((note: { text: string }) => note.text)).toContain(
    appviewText,
  );
  const sinceRequests = await request.get(
    `${APPVIEW_CONTROL_URL}/__control__/search/requests`,
  );
  expect(await sinceRequests.json()).toEqual([]);
});

// 検索はローカルDBとBsky AppViewを並行検索してブレンドする
// (crates/seiran-api/src/handlers/search.rs)。AppViewはstub-appview-serverが常に
// 空を返すため、ここではローカル投稿がヒットすることだけを検証する。
test("投稿本文で検索できる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2esearch");
  const keyword = `検索キーワード${Date.now()}`;
  const text = `${keyword} を含む投稿`;

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${user.token}` },
    data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

  await seedAuth(page, user.token);
  await page.goto("/search");
  await page.getByPlaceholder("キーワードを検索（ローカル + Bluesky）").fill(keyword);
  await page.getByRole("button", { name: "検索", exact: true }).click();

  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });
});

test("検索式（語句・グループ・縦棒の OR・除外）をローカル投稿にも適用し、指定子の形はただの語にする", async ({
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2esearchopsa");
  const bob = await registerUserViaApi(request, "e2esearchopsb");
  const unique = `queryops${Date.now()}`;
  const matchingText = `${unique} exact phrase #rust from:literal`;
  const excludedText = `${unique} exact phrase spam`;

  for (const [user, text] of [[alice, matchingText], [bob, excludedText]] as const) {
    const response = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
    });
    expect(response.ok(), `create failed: ${response.status()} ${await response.text()}`).toBeTruthy();
  }

  const search = async (q: string) => {
    const response = await request.get(`/api/notes/search?q=${encodeURIComponent(q)}`, {
      headers: { Authorization: `Bearer ${alice.token}` },
    });
    expect(response.ok(), `search failed: ${response.status()} ${await response.text()}`).toBeTruthy();
    return ((await response.json()) as { notes: { text: string }[] }).notes.map((note) => note.text);
  };

  const filtered = await search(`("exact phrase" | absent) -spam ${unique}`);
  expect(filtered).toContain(matchingText);
  expect(filtered).not.toContain(excludedText);

  const unbalanced = await search(`(${unique} | definitely-absent`);
  expect(unbalanced).toEqual(expect.arrayContaining([matchingText, excludedText]));

  // `from:` は投稿者の指定ではなく、本文の語として探す。
  expect(await search(`from:literal ${unique}`)).toEqual([matchingText]);
});

// 検索結果もタイムラインと同じくリアクション・引用元の埋め込みが付くこと。
test("検索結果にもリアクションと引用元の埋め込みが付く", async ({ request }) => {
  const clear = await request.post(`${APPVIEW_CONTROL_URL}/__control__/search`, {
    data: { posts: [] },
  });
  expect(clear.ok(), await clear.text()).toBeTruthy();

  const user = await registerUserViaApi(request, "e2esearchembed");
  const unique = `searchembed${Date.now().toString(36)}`;
  const auth = { Authorization: `Bearer ${user.token}` };

  const originalRes = await request.post("/api/notes/create", {
    headers: auth,
    data: { text: "引用元の投稿", deliver_to_fedi: false, deliver_to_bsky: false },
  });
  expect(originalRes.ok(), await originalRes.text()).toBeTruthy();
  const original = await originalRes.json();

  const quoteRes = await request.post("/api/notes/create", {
    headers: auth,
    data: {
      text: `${unique} 引用投稿`,
      quote_of_id: original.id,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
    },
  });
  expect(quoteRes.ok(), await quoteRes.text()).toBeTruthy();
  const quote = await quoteRes.json();

  const reactRes = await request.post(`/api/notes/${quote.id}/reactions`, {
    headers: auth,
    data: { content: "👍" },
  });
  expect(reactRes.ok(), await reactRes.text()).toBeTruthy();

  const searchRes = await request.get(`/api/notes/search?q=${encodeURIComponent(unique)}`, {
    headers: auth,
  });
  expect(searchRes.ok(), await searchRes.text()).toBeTruthy();
  const { notes } = await searchRes.json();
  const hit = notes.find((n: { id: string }) => n.id === quote.id);
  expect(hit, "引用投稿が検索結果に出ない").toBeTruthy();
  expect(hit.quote?.id).toBe(original.id);
  expect(hit.reactions).toEqual(
    expect.arrayContaining([expect.objectContaining({ emoji: "👍", count: 1, reactedByMe: true })]),
  );
});

// 検索はログイン必須（crates/seiran-api/src/handlers/search.rs）。全文検索はクエリによって
// 重く、AppView の結果の保存も伴うので、回数制限の掛からない未ログインには開かない。
test("未ログインでは投稿もユーザーも検索できない", async ({ request }) => {
  const custom = await request.get("/api/notes/search?q=hello");
  expect(custom.status()).toBe(401);
  const customUsers = await request.get("/api/users/search?q=hello");
  expect(customUsers.status()).toBe(401);

  const misskey = await request.post("/api/notes/search", { data: { query: "hello" } });
  expect(misskey.status()).toBe(401);
  const misskeyUsers = await request.post("/api/users/search", { data: { query: "hello" } });
  expect(misskeyUsers.status()).toBe(401);

  // Mastodon 互換はハッシュタグの結果を返せるよう、エラーにせず投稿とアカウントを空にする
  const mastodon = await request.get("/api/v2/search?q=hello");
  expect(mastodon.ok(), await mastodon.text()).toBeTruthy();
  const body = (await mastodon.json()) as { statuses: unknown[]; accounts: unknown[]; hashtags: unknown[] };
  expect(body.statuses).toEqual([]);
  expect(body.accounts).toEqual([]);
  expect(body.hashtags.length).toBe(1);
});

test("検索語とは別の条件（ハッシュタグ・サーバー・ユーザー）で絞り込める", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2esearchcond");
  const viewers = [
    await registerUserViaApi(request, "e2esearchcondv1"),
    await registerUserViaApi(request, "e2esearchcondv2"),
  ];
  const tag = `condtag${Date.now().toString(36)}`;
  const localText = `ローカルの投稿 #${tag}`;
  const bskyText = `Blueskyの投稿 #${tag}`;
  const did = `did:plc:e2econd${Date.now().toString(36)}`;

  const create = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: localText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(create.ok(), await create.text()).toBeTruthy();

  // AppView スタブは条件を見ずにこの投稿を返す。サーバーに Bluesky 以外を指定したときは
  // AppView に問い合わせないので、ローカルの投稿だけになる。
  const configure = await request.post(`${APPVIEW_CONTROL_URL}/__control__/search`, {
    data: {
      posts: [
        {
          uri: `at://${did}/app.bsky.feed.post/cond`,
          cid: `bafy${Date.now().toString(36)}`,
          text: bskyText,
          createdAt: new Date(Date.now() - 60_000).toISOString(),
          author: { did, handle: `cond-${Date.now().toString(36)}.bsky.social`, displayName: "Cond" },
        },
      ],
    },
  });
  expect(configure.ok(), await configure.text()).toBeTruthy();

  let calls = 0;
  const search = async (params: Record<string, string>) => {
    // 検索の回数制限にかからないよう、閲覧者を2回ごとに替える。
    const viewer = viewers[Math.floor(calls++ / 2)];
    const response = await request.get(`/api/notes/search?${new URLSearchParams(params)}`, {
      headers: { Authorization: `Bearer ${viewer.token}` },
    });
    expect(response.ok(), await response.text()).toBeTruthy();
    return ((await response.json()) as { notes: { text: string }[] }).notes.map((n) => n.text);
  };

  // 検索語なしでハッシュタグだけ。
  expect(await search({ tag: `#${tag}` })).toEqual(expect.arrayContaining([localText, bskyText]));
  expect(await search({ tag, server: "local" })).toEqual([localText]);
  expect(await search({ tag, server: "bluesky" })).toEqual([bskyText]);
  expect(await search({ q: "投稿", user_id: alice.actorId })).toEqual([localText]);
});
