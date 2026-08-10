import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// ポスト詳細画面の右ペインタブ選択状態の永続化（#226）。
test("タブ選択状態はURLに反映されリロード後も保たれる", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2etabpersistauth");
  const text = `タブ永続化確認 ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const note = await createRes.json();

  await seedAuth(page, author.token);
  await page.goto(`/notes/${note.id}`);
  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });

  const rightPane = page.getByRole("complementary");
  await rightPane.getByRole("button", { name: "リアクション", exact: true }).click();
  await expect(page).toHaveURL(/[?&]tab=3(&|$)/);

  await page.reload();
  await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });
  // リロード後もリアクションタブがアクティブなまま（リアクション無しの空状態文言で確認）。
  await expect(
    page.getByRole("complementary").getByText("このポストにはまだリアクションがありません。"),
  ).toBeVisible({ timeout: 15_000 });
});

// ポスト詳細画面「前後のポスト」タブのスクロール位置復元（#226）。
test("前後のポストのスクロール位置はブラウザバックで復元される", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ectxscrollauth");
  const ts = Date.now();

  async function post(text: string) {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${author.token}` },
      data: { text, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
    });
    expect(res.ok(), `create failed: ${res.status()} ${await res.text()}`).toBeTruthy();
    return res.json();
  }

  for (let i = 1; i <= 5; i++) await post(`古い投稿${i} ${ts}`);
  const target = await post(`対象ポスト ${ts}`);
  for (let i = 1; i <= 5; i++) await post(`新しい投稿${i} ${ts}`);

  await seedAuth(page, author.token);
  await page.goto(`/notes/${target.id}`);
  await expect(page.getByText(`対象ポスト ${ts}`)).toBeVisible({ timeout: 15_000 });

  const rightPane = page.getByRole("complementary");
  await rightPane.getByRole("button", { name: "前後のポスト", exact: true }).click();
  // 対象ポスト自身へ自動スクロール（初回、記憶済み位置が無いため）されるのを待つ。
  await expect(rightPane.getByText(`対象ポスト ${ts}`)).toBeVisible({ timeout: 15_000 });

  // 先頭（最も新しい投稿の上）まで明示的にスクロールする。
  await rightPane.evaluate((el) => {
    el.scrollTop = 0;
  });
  await page.waitForTimeout(300);
  const scrollBefore = await rightPane.evaluate((el) => el.scrollTop);
  expect(scrollBefore).toBe(0);

  // SPA内ナビゲーション（ホームへ）→ ブラウザバックで戻る。
  await page.getByRole("link", { name: /ホーム/ }).first().click();
  await expect(page).toHaveURL(/\/$/, { timeout: 15_000 });
  await page.goBack();
  await expect(page.getByRole("main").getByText(`対象ポスト ${ts}`)).toBeVisible({ timeout: 15_000 });

  // 記憶していたスクロール位置（先頭付近）が復元され、対象ポスト中央寄せの
  // デフォルト位置（スクロールされ直す）にはならない。
  await expect
    .poll(async () => rightPane.evaluate((el) => el.scrollTop), { timeout: 15_000 })
    .toBeLessThan(50);
});
