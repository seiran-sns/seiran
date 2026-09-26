import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("ハッシュタグ付き投稿がハッシュタグタイムラインに表示される", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2ehashtag");
  const tag = `e2etag${Date.now().toString(36)}`;
  const text = `${tag}のテスト投稿 #${tag}`;

  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${user.token}` },
    data: {
      text,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
      visibility: "public",
    },
  });
  expect(
    createRes.ok(),
    `create failed: ${createRes.status()} ${await createRes.text()}`,
  ).toBeTruthy();

  await seedAuth(page, user.token);
  await page.goto(`/tags/${tag}`);

  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });
});

test("Misskey互換APIでハッシュタグ付き投稿を取得できる", async ({
  request,
}) => {
  const user = await registerUserViaApi(request, "e2emisskeytag");
  const tag = `e2emisskeytag${Date.now().toString(36)}`;
  const text = `Misskey互換ハッシュタグ検索 #${tag}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${user.token}` },
    data: {
      text,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
      visibility: "public",
    },
  });
  expect(
    createRes.ok(),
    `create failed: ${createRes.status()} ${await createRes.text()}`,
  ).toBeTruthy();

  const response = await request.post("/api/notes/search-by-tag", {
    headers: { Authorization: `Bearer ${user.token}` },
    data: { tag: `#${tag.toUpperCase()}`, limit: 10 },
  });
  expect(
    response.ok(),
    `search-by-tag failed: ${response.status()} ${await response.text()}`,
  ).toBeTruthy();
  const notes = (await response.json()) as { text: string }[];
  expect(notes.map((note) => note.text)).toContain(text);
});

test("ハッシュタグをホーム画面に追加・削除できる", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2ehashtagpin");
  const tag = `e2epin${Date.now().toString(36)}`;

  await seedAuth(page, user.token);
  await page.goto(`/tags/${tag}`);

  await page
    .getByRole("button", { name: "ホーム画面に追加", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "ホーム画面から削除", exact: true }),
  ).toBeVisible({
    timeout: 15_000,
  });

  await page
    .getByRole("button", { name: "ホーム画面から削除", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "ホーム画面に追加", exact: true }),
  ).toBeVisible({
    timeout: 15_000,
  });
});

// ノート組み立ての共通化（`build_note_responses`）以前は、ハッシュタグTLだけ投票済み状態
// （`poll.votedByMe`）を付与しておらず、投票後もTL上では未投票に見えていた。
test("ハッシュタグタイムラインでも自分の投票済み状態が返る", async ({ request }) => {
  const user = await registerUserViaApi(request, "e2etagpoll");
  const tag = `e2etagpoll${Date.now().toString(36)}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${user.token}` },
    data: {
      text: `投票 #${tag}`,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
      poll: { choices: ["A", "B"], expiresInSeconds: 3600 },
    },
  });
  expect(createRes.ok(), await createRes.text()).toBeTruthy();
  const created = await createRes.json();

  const voteRes = await request.post(`/api/notes/${created.id}/poll-vote`, {
    headers: { Authorization: `Bearer ${user.token}` },
    data: { optionIndexes: [1] },
  });
  expect(voteRes.ok(), await voteRes.text()).toBeTruthy();

  const tlRes = await request.get(`/api/hashtags/${tag}/timeline`, {
    headers: { Authorization: `Bearer ${user.token}` },
  });
  expect(tlRes.ok(), await tlRes.text()).toBeTruthy();
  const notes = await tlRes.json();
  const note = notes.find((n: { id: string }) => n.id === created.id);
  expect(note, "投稿がハッシュタグTLに出ない").toBeTruthy();
  expect(note.poll.votedByMe).toEqual([1]);
  expect(note.repostedByMe).toBe(false);
});
