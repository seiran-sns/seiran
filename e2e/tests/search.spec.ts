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

test("Bluesky互換の検索式をローカル投稿にも適用する", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2esearchopsa");
  const bob = await registerUserViaApi(request, "e2esearchopsb");
  const unique = `queryops${Date.now()}`;
  const matchingText = `${unique} exact phrase #rust https://example.com/article @${bob.username}`;
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
    return (await response.json()) as { notes: { text: string }[] };
  };

  const filtered = await search(
    `from:me ("exact phrase" OR absent) -spam domain:example.com lang:ja ${unique}`,
  );
  expect(filtered.notes.map((note) => note.text)).toContain(matchingText);
  expect(filtered.notes.map((note) => note.text)).not.toContain(excludedText);

  const unbalanced = await search(`(${unique} OR definitely-absent`);
  expect(unbalanced.notes.map((note) => note.text)).toEqual(
    expect.arrayContaining([matchingText, excludedText]),
  );

  const mention = await search(`mentions:${bob.username} ${unique}`);
  expect(mention.notes.map((note) => note.text)).toContain(matchingText);
});
