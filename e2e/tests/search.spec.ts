import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

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
