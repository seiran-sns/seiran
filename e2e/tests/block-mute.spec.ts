import { test, expect, Page } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

async function openActionsMenu(page: Page) {
  await page.getByTitle("アカウント操作").click();
}

test("ブロックするとフォロー関係が解除され、投稿が相互に非表示になる", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2eblocka");
  const bob = await registerUserViaApi(request, "e2eblockb");

  const followRes = await request.post("/api/follows/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(followRes.ok(), `フォロー失敗: ${followRes.status()} ${await followRes.text()}`).toBeTruthy();

  const postRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { text: `ブロックテスト投稿 ${Date.now()}` },
  });
  expect(postRes.ok(), `投稿失敗: ${postRes.status()} ${await postRes.text()}`).toBeTruthy();

  await seedAuth(page, alice.token);
  await page.goto(`/@${bob.username}`);
  await expect(page.getByText("フォロー中")).toBeVisible({ timeout: 15_000 });

  await openActionsMenu(page);
  await page.getByRole("button", { name: "ブロック", exact: true }).click();
  await page.getByRole("button", { name: "ブロックする" }).click();

  // ブロック実行によりフォロー関係が強制解除される。
  await expect(page.getByText("フォロー中")).not.toBeVisible({ timeout: 15_000 });

  // ブロック後は相手の投稿が非表示になる（actor_is_hidden_for_viewer によるタイムラインフィルタ）。
  await page.reload();
  await expect(page.getByText("投稿がありません。")).toBeVisible({ timeout: 15_000 });

  // ブロック解除で投稿が再び見えるようになる。
  await openActionsMenu(page);
  await page.getByRole("button", { name: "ブロック解除" }).click();
  await expect(page.getByText(/ブロックテスト投稿/)).toBeVisible({ timeout: 15_000 });
});

test("ブロック中は相手をフォローできない", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2eblkfola");
  const bob = await registerUserViaApi(request, "e2eblkfolb");

  const blockRes = await request.post("/api/blocks/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(blockRes.ok(), `ブロック失敗: ${blockRes.status()} ${await blockRes.text()}`).toBeTruthy();

  const followRes = await request.post("/api/follows/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { target: bob.username },
  });
  expect(followRes.status(), "ブロック中のフォローは拒否されるべき").toBe(403);

  // UI上も独立ボタンのフォローが無効化されていることを確認する
  // （フォローボタンはメニュー内にも統合されているが、独立ボタンとしても残っているため
  // メニューを開くと同じラベルの要素が2つになり strict mode 違反になる。開かずに確認する）。
  await seedAuth(page, alice.token);
  await page.goto(`/@${bob.username}`);
  await expect(page.getByRole("button", { name: "フォロー", exact: true })).toBeDisabled();
});

test("ミュートすると相手からの通知が届かなくなる", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2emutea");
  const bob = await registerUserViaApi(request, "e2emuteb");

  const postRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: `ミュートテスト ${Date.now()}` },
  });
  expect(postRes.ok(), `投稿失敗: ${postRes.status()} ${await postRes.text()}`).toBeTruthy();
  const created = await postRes.json();

  await seedAuth(page, alice.token);
  await page.goto(`/@${bob.username}`);
  await openActionsMenu(page);
  await page.getByRole("button", { name: "ミュート", exact: true }).click();

  const reactRes = await request.post(`/api/notes/${created.id}/reactions`, {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { content: "🎉" },
  });
  expect(reactRes.ok(), `リアクション失敗: ${reactRes.status()} ${await reactRes.text()}`).toBeTruthy();

  await page.goto("/notifications");
  await expect(page.getByText("新しい通知はありません。")).toBeVisible({ timeout: 10_000 });
});
