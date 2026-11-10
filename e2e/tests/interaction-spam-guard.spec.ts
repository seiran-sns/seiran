import { test, expect, APIRequestContext } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// リポスト・リアクションの連打対策（docs/architecture.md 4節「リポスト・リアクションの連打対策」）。
// 送信元は同じ働きかけを10秒間 429 で止め、受信側は同じ相手からの同じ働きかけの通知を5分間
// 作らない。クールダウン明けを確かめるテストは実時間で10秒待つ（バックエンドの判定は
// 実時間なので、ブラウザ側の時計を進めても再現できない）。

const COOLDOWN_MS = 10_000;
/** クールダウン明けを待つ上限（10秒＋処理の揺らぎ）。 */
const AFTER_COOLDOWN_TIMEOUT = COOLDOWN_MS + 8_000;

type Notification = {
  type: string;
  reaction?: string;
  repostWithdrawn?: boolean;
  note?: { id: string; renote?: { id: string } };
};

async function createNote(request: APIRequestContext, token: string, text: string) {
  const res = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${token}` },
    data: { text },
  });
  expect(res.ok(), `投稿作成失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()) as { id: string };
}

function repost(request: APIRequestContext, token: string, noteId: string) {
  return request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${token}` },
    data: { renote_id: noteId },
  });
}

async function unrepost(request: APIRequestContext, token: string, noteId: string) {
  const res = await request.delete(`/api/notes/${noteId}/repost`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  expect(res.ok(), `リポスト取消失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
}

function react(request: APIRequestContext, token: string, noteId: string, content: string) {
  return request.post(`/api/notes/${noteId}/reactions`, {
    headers: { Authorization: `Bearer ${token}` },
    data: { content },
  });
}

async function notifications(request: APIRequestContext, token: string): Promise<Notification[]> {
  const res = await request.post("/api/i/notifications", {
    headers: { Authorization: `Bearer ${token}` },
    data: { limit: 50 },
  });
  expect(res.ok(), `通知取得失敗: ${res.status()}`).toBeTruthy();
  return (await res.json()) as Notification[];
}

test("同じポストの再リポストは10秒間429で止まり、明けた後の再リポストも通知は1件のまま", async ({
  request,
}) => {
  const author = await registerUserViaApi(request, "e2espamrpa");
  const reposter = await registerUserViaApi(request, "e2espamrpr");
  const original = await createNote(request, author.token, `連打対策リポスト ${Date.now()}`);
  const other = await createNote(request, author.token, `連打対策リポスト別 ${Date.now()}`);

  const first = await repost(request, reposter.token, original.id);
  expect(first.ok(), `リポスト失敗: ${first.status()} ${await first.text()}`).toBeTruthy();
  await unrepost(request, reposter.token, original.id);

  const again = await repost(request, reposter.token, original.id);
  expect(again.status()).toBe(429);
  expect((await again.json()).code).toBe("REPOST_COOLDOWN");
  expect(Number(again.headers()["retry-after"])).toBeGreaterThan(0);

  // 別のポストは止めない。
  const otherRes = await repost(request, reposter.token, other.id);
  expect(otherRes.ok(), `別ポストのリポスト失敗: ${otherRes.status()}`).toBeTruthy();

  // クールダウンが明ければリポストできる。
  await expect
    .poll(async () => (await repost(request, reposter.token, original.id)).status(), {
      timeout: AFTER_COOLDOWN_TIMEOUT,
      intervals: [2_000],
    })
    .toBe(200);

  // 受信側の抑制（5分）で、同じ元ポストのリポスト通知は最初の1件だけ。
  const renotes = (await notifications(request, author.token)).filter(
    (n) => n.type === "renote" && (n.note?.renote?.id ?? n.note?.id) === original.id,
  );
  expect(renotes).toHaveLength(1);
  // 最初のラッパーは取り消し済みなので、その通知はリポスト元へたどる印を持つ。
  expect(renotes[0].repostWithdrawn).toBe(true);
});

test("同じリアクションは10秒間429で止まり、違う絵文字は止まらない。通知は絵文字ごとに1件", async ({
  request,
}) => {
  const author = await registerUserViaApi(request, "e2espamrxa");
  const reactor = await registerUserViaApi(request, "e2espamrxr");
  const note = await createNote(request, author.token, `連打対策リアクション ${Date.now()}`);

  expect((await react(request, reactor.token, note.id, "👍")).ok()).toBeTruthy();
  // 絵文字を続けて付けて文にする使い方があるので、別の絵文字は通す（1人1個なので切り替わる）。
  expect((await react(request, reactor.token, note.id, "🎉")).ok()).toBeTruthy();

  const again = await react(request, reactor.token, note.id, "👍");
  expect(again.status()).toBe(429);
  expect((await again.json()).code).toBe("REACTION_COOLDOWN");

  await expect
    .poll(async () => (await react(request, reactor.token, note.id, "👍")).status(), {
      timeout: AFTER_COOLDOWN_TIMEOUT,
      intervals: [2_000],
    })
    .toBe(200);

  const reactions = (await notifications(request, author.token)).filter(
    (n) => n.type === "reaction" && n.note?.id === note.id,
  );
  expect(reactions.filter((n) => n.reaction === "👍")).toHaveLength(1);
  expect(reactions.filter((n) => n.reaction === "🎉")).toHaveLength(1);
});

test("リポストを取り消すと10秒間リポストボタンが無効になり、明けると押せる", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2espamuia");
  const reposter = await registerUserViaApi(request, "e2espamuir");
  await createNote(request, author.token, `連打対策ボタン ${Date.now()}`);

  await seedAuth(page, reposter.token);
  await page.goto(`/@${author.username}`);

  const repostButton = page.getByTitle("リポスト", { exact: true });
  await expect(repostButton).toBeVisible({ timeout: 15_000 });
  await repostButton.click();
  const unrepostButton = page.getByTitle("リポスト解除", { exact: true });
  await expect(unrepostButton).toBeVisible({ timeout: 10_000 });
  // 取り消しはクールダウン中でもできる。
  await expect(unrepostButton).toBeEnabled();
  await unrepostButton.click();

  const cooldownButton = page.getByTitle("続けてリポストすることはできません。少し待ってください", {
    exact: true,
  });
  await expect(cooldownButton).toBeVisible({ timeout: 10_000 });
  await expect(cooldownButton).toBeDisabled();

  await expect(repostButton).toBeEnabled({ timeout: AFTER_COOLDOWN_TIMEOUT });
  await repostButton.click();
  await expect(unrepostButton).toBeVisible({ timeout: 10_000 });
});

test("同じ絵文字のリアクションを付け直すと10秒間はエラートーストで止まる", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2espamuxa");
  const reactor = await registerUserViaApi(request, "e2espamuxr");
  const someone = await registerUserViaApi(request, "e2espamuxs");
  const text = `連打対策リアクションUI ${Date.now()}`;
  const note = await createNote(request, author.token, text);
  // 押せるリアクションチップを用意する。
  expect((await react(request, someone.token, note.id, "👍")).ok()).toBeTruthy();

  await seedAuth(page, reactor.token);
  await page.goto(`/notes/${note.id}`);
  const card = page.locator("article").filter({ hasText: text }).first();
  await expect(card).toBeVisible({ timeout: 15_000 });

  await card.getByRole("button", { name: /👍\s*1/ }).click();
  await expect(card.getByRole("button", { name: /👍\s*2/ })).toBeVisible({ timeout: 10_000 });
  await card.getByRole("button", { name: /👍\s*2/ }).click();
  await expect(card.getByRole("button", { name: /👍\s*1/ })).toBeVisible({ timeout: 10_000 });

  await card.getByRole("button", { name: /👍\s*1/ }).click();
  await expect(
    page.getByText("同じリアクションを続けて付けることはできません。少し待ってから再試行してください"),
  ).toBeVisible({ timeout: 10_000 });
  // 送っていないので件数は変わらない。
  await expect(card.getByRole("button", { name: /👍\s*1/ })).toBeVisible();

  // クールダウンが明ければ付けられる。
  await expect
    .poll(
      async () => {
        await card.getByRole("button", { name: /👍\s*\d/ }).click();
        return card.getByRole("button", { name: /👍\s*2/ }).isVisible();
      },
      { timeout: AFTER_COOLDOWN_TIMEOUT, intervals: [3_000] },
    )
    .toBe(true);
});

test("取り消されたリポストの通知をクリックするとリポスト元へ遷移する", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2espamnva");
  const reposter = await registerUserViaApi(request, "e2espamnvr");
  const original = await createNote(request, author.token, `取り消しリポスト通知 ${Date.now()}`);

  const res = await repost(request, reposter.token, original.id);
  expect(res.ok(), `リポスト失敗: ${res.status()}`).toBeTruthy();
  await unrepost(request, reposter.token, original.id);

  await seedAuth(page, author.token);
  await page.goto("/notifications");
  await page.getByText(`${reposter.username} がリポストしました`).click();
  await expect(page).toHaveURL(new RegExp(`/notes/${original.id}$`), { timeout: 10_000 });
});
