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
