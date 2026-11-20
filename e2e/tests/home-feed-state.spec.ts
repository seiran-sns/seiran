import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("ホーム画面から他画面へ遷移して戻ると、選択タブとスクロール位置が保持される", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2ehomestate");

  // スクロールが発生するよう十分な件数の投稿を作る。
  for (let i = 0; i < 25; i++) {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { text: `ホーム状態保持テスト投稿 ${i} ${Date.now()}` },
    });
    expect(res.ok(), `投稿作成失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  }

  await seedAuth(page, user.token);
  await page.goto("/");

  // ローカルタブへ切り替える。
  await page.getByRole("button", { name: "ローカル", exact: true }).click();
  await expect(page.getByRole("button", { name: "ローカル", exact: true })).toHaveClass(
    /feedTabActive/,
  );

  // 投稿一覧が描画されるのを待ってからスクロールする。
  await expect(page.locator("article").first()).toBeVisible({ timeout: 15_000 });
  await page.evaluate(() => window.scrollTo(0, 400));
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(200);
  const scrollYBefore = await page.evaluate(() => window.scrollY);

  // 別画面（通知）へ遷移してからホームへ戻る（SPA内遷移、フルリロードなし）。
  await page.getByRole("link", { name: /通知/ }).click();
  await expect(page).toHaveURL(/\/notifications$/);
  await page.getByRole("link", { name: /ホーム/ }).click();
  await expect(page).toHaveURL(/\/$/);

  // タブ選択がローカルのまま保持されている。
  await expect(page.getByRole("button", { name: "ローカル", exact: true })).toHaveClass(
    /feedTabActive/,
    { timeout: 10_000 },
  );

  // スクロール位置も復元されている（キャッシュ復元後にrAFで反映されるため多少の遅延を許容）。
  // 復元直後は画像読み込み等に伴うブラウザのscroll anchoringでscrollYがわずかに動くことが
  // あるため、しきい値超えを検出した瞬間の値ではなく、値が安定してから比較する
  // （フルスイート実行時のみ稀に失敗していたflaky対策）。
  await expect(async () => {
    const y1 = await page.evaluate(() => window.scrollY);
    expect(y1).toBeGreaterThan(200);
    await page.waitForTimeout(200);
    const y2 = await page.evaluate(() => window.scrollY);
    expect(y2).toBe(y1);
  }).toPass({ timeout: 10_000 });
  const scrollYAfter = await page.evaluate(() => window.scrollY);
  expect(Math.abs(scrollYAfter - scrollYBefore)).toBeLessThan(50);
});

test("トップにいない間の新しいポストは一覧に入れず件数で知らせ、押すとトップへ戻って入る", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2ehomepend");
  for (let i = 0; i < 25; i++) {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { text: `新着保留テスト投稿 ${i} ${Date.now()}` },
    });
    expect(res.ok(), `投稿作成失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  }

  await seedAuth(page, user.token);
  await page.goto("/");
  await page.getByRole("button", { name: "ホーム", exact: true }).click();
  await expect(page.locator("article").first()).toBeVisible({ timeout: 15_000 });
  await page.evaluate(() => window.scrollTo(0, 600));
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(400);
  const scrollYBefore = await page.evaluate(() => window.scrollY);

  const text = `新着保留テスト 最新 ${Date.now()}`;
  const pill = page.getByRole("button", { name: /新しいポスト \d+件/ });
  // WebSocket の購読が張られる前に投稿すると届かないので、届くまで投稿し直す。
  await expect(async () => {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { text },
    });
    expect(res.ok()).toBeTruthy();
    await expect(pill).toBeVisible({ timeout: 3_000 });
  }).toPass({ timeout: 20_000 });

  // 一覧は変わらず、見ていた位置もそのまま。
  await expect(page.locator("article").filter({ hasText: text })).toHaveCount(0);
  expect(Math.abs((await page.evaluate(() => window.scrollY)) - scrollYBefore)).toBeLessThan(2);

  await pill.click();
  await expect.poll(() => page.evaluate(() => window.scrollY), { timeout: 10_000 }).toBe(0);
  await expect(page.locator("article").first()).toContainText(text, { timeout: 10_000 });
  await expect(pill).toHaveCount(0);
});

test("ブラウザバックで戻ると、上のポストの高さが変わっていても同じポストが同じ位置に戻る", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2ehomeback");
  for (let i = 0; i < 25; i++) {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { text: `戻る位置テスト投稿 ${i} ${Date.now()}` },
    });
    expect(res.ok(), `投稿作成失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  }

  await seedAuth(page, user.token);
  await page.goto("/");
  await page.getByRole("button", { name: "ホーム", exact: true }).click();
  await expect(page.locator("[data-timeline-id]").nth(20)).toBeAttached({ timeout: 15_000 });
  await page.evaluate(() => window.scrollTo(0, 1500));
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(1000);

  // 一番上に見えているポストと、その上端の位置。
  const topmost = () =>
    page.evaluate(() => {
      const cover = document.querySelector('[class*="feedTabs"]')?.getBoundingClientRect().bottom ?? 0;
      const el = [...document.querySelectorAll<HTMLElement>("[data-timeline-id]")].find(
        (e) => e.getBoundingClientRect().bottom > cover,
      );
      return { id: el?.dataset.timelineId, top: Math.round(el?.getBoundingClientRect().top ?? 0) };
    });
  // 画面内に上端があるポストの詳細へ（クリックでスクロールさせない）。
  const targetId = await page.evaluate(() => {
    const el = [...document.querySelectorAll<HTMLElement>("[data-timeline-id]")].find((e) => {
      const r = e.getBoundingClientRect();
      return r.top > 120 && r.top < window.innerHeight - 200;
    });
    return el?.dataset.timelineId;
  });
  const before = await topmost();
  await page.locator(`[data-timeline-id="${targetId}"] a[href^="/notes/"]`).first().click();
  await expect(page).toHaveURL(/\/notes\//);

  // 戻ったときに先頭のポストだけ高くなるようにする（画像の読み込み等で上の高さが変わった状況）。
  // ブラウザ標準のスクロール復元は画素位置を再現するので、これが効いているとずれる。
  await page.addStyleTag({
    content: "div:has(> [data-timeline-id]):first-child > [data-timeline-id] { padding-top: 600px !important; }",
  });
  await page.goBack();
  await expect(page).toHaveURL(/\/$/);
  await expect.poll(topmost, { timeout: 10_000 }).toEqual(before);
});

test("タイムラインの選択タブはリロード後も保持される", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2ehometab");
  await seedAuth(page, user.token);
  await page.goto("/");

  await page.getByRole("button", { name: "グローバル", exact: true }).click();
  await expect(page.getByRole("button", { name: "グローバル", exact: true })).toHaveClass(
    /feedTabActive/,
  );

  await page.reload();
  await expect(page.getByRole("button", { name: "グローバル", exact: true })).toHaveClass(
    /feedTabActive/,
    { timeout: 10_000 },
  );
});
