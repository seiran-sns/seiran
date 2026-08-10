import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// ポスト詳細画面の右ペイン「リアクション」タブ（#226）: 絵文字ごとにグループ化した
// アクター一覧をホバー不要で常時展開表示する。
test("ポスト詳細のリアクションタブに絵文字ごとのリアクター一覧が表示される", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ereacttabauth");
  const reactor = await registerUserViaApi(request, "e2ereacttabusr");

  const text = `リアクションタブ確認用 ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const note = await createRes.json();

  const reactRes = await request.post(`/api/notes/${note.id}/reactions`, {
    headers: { Authorization: `Bearer ${reactor.token}` },
    data: { content: "👍" },
  });
  expect(reactRes.ok(), `react failed: ${reactRes.status()} ${await reactRes.text()}`).toBeTruthy();

  await seedAuth(page, author.token);
  await page.goto(`/notes/${note.id}`);
  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });

  const rightPane = page.getByRole("complementary");
  await rightPane.getByRole("button", { name: "リアクション", exact: true }).click();
  await expect(rightPane.getByText(reactor.username)).toBeVisible({ timeout: 15_000 });
});
