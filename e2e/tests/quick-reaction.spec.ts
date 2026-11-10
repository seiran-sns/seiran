import { test, expect, APIRequestContext, CDPSession, Locator } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// NoteCard の🙂ボタンの右に並ぶクイックリアクションボタン: 先頭は常に❤️、続いて自分が
// よく使う絵文字（1マス幅のもの）を頻度順に最大3つ。

async function createNote(request: APIRequestContext, token: string, text: string) {
  const res = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${token}` },
    data: { text },
  });
  expect(res.ok(), `create failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()) as { id: string };
}

async function react(request: APIRequestContext, token: string, noteId: string, content: string) {
  const res = await request.post(`/api/notes/${noteId}/reactions`, {
    headers: { Authorization: `Bearer ${token}` },
    data: { content },
  });
  expect(res.ok(), `react failed: ${res.status()} ${await res.text()}`).toBeTruthy();
}

/** 1投稿1リアクションなので、使用回数ぶん別々のポストへ付けて「よく使う絵文字」を作る。 */
async function seedFrequentReactions(request: APIRequestContext, token: string, prefix: string) {
  // 👍3・🎉2・❤️2（❤️は固定枠なので重複させない）・🙏1・😂1（同数なら新しい方が上）。
  const usage = ["👍", "👍", "👍", "🎉", "🎉", "❤️", "❤️", "🙏", "😂"];
  for (const [i, emoji] of usage.entries()) {
    const note = await createNote(request, token, `${prefix} 下準備 ${i}`);
    await react(request, token, note.id, emoji);
  }
}

function quickTitles(card: Locator) {
  return card.locator('button[title$=" でリアクション"], button[title$=" のリアクションを取り消す"]');
}

test("❤️のクイックリアクションボタンで付けて、青枠になったボタンで取り消せる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2equickheart");
  const text = `クイックリアクションいいね ${Date.now()}`;
  await createNote(request, user.token, text);

  await seedAuth(page, user.token);
  await page.goto("/");
  const card = page.locator("article").filter({ hasText: text }).first();
  await expect(card).toBeVisible({ timeout: 15_000 });

  // まだ何も使っていないユーザーには❤️だけが並ぶ。
  await expect(quickTitles(card)).toHaveCount(1);
  await card.getByTitle("❤️ でリアクション", { exact: true }).click();

  await expect(card.getByRole("button", { name: /❤️\s*1/ })).toBeVisible({ timeout: 15_000 });
  const removeButton = card.getByTitle("❤️ のリアクションを取り消す", { exact: true });
  await expect(removeButton).toHaveClass(/actionBtnActive/);
  await expect(card.getByTitle("リアクションを付ける", { exact: true })).toHaveClass(/actionBtnActive/);

  await removeButton.click();
  await expect(card.getByRole("button", { name: /❤️\s*\d/ })).toHaveCount(0, { timeout: 15_000 });
  await expect(card.getByTitle("❤️ でリアクション", { exact: true })).not.toHaveClass(/actionBtnActive/);
  await expect(card.getByTitle("リアクションを付ける", { exact: true })).not.toHaveClass(/actionBtnActive/);
});

test("よく使う絵文字が❤️の後ろに頻度順で並び、別の絵文字のボタンで付け替えられる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2equickfreq");
  const prefix = `クイック頻度 ${Date.now()}`;
  await seedFrequentReactions(request, user.token, prefix);
  const text = `${prefix} 対象`;
  await createNote(request, user.token, text);

  await page.setViewportSize({ width: 1600, height: 900 });
  await seedAuth(page, user.token);
  await page.goto("/");
  const card = page.locator("article").filter({ hasText: text }).first();
  await expect(card).toBeVisible({ timeout: 15_000 });

  await expect(quickTitles(card)).toHaveCount(4, { timeout: 15_000 });
  await expect
    .poll(() => quickTitles(card).evaluateAll((els) => els.map((e) => e.getAttribute("title"))))
    .toEqual(["❤️ でリアクション", "👍 でリアクション", "🎉 でリアクション", "😂 でリアクション"]);

  // ❤️以外はカードにホバーしている間だけ見える。表示が切り替わってもメニューの位置は動かない。
  const menu = card.getByRole("button", { name: "⋯" });
  await page.mouse.move(0, 0);
  await expect(card.getByTitle("❤️ でリアクション", { exact: true })).toBeVisible();
  await expect(card.getByTitle("👍 でリアクション", { exact: true })).toBeHidden();
  const menuBoxBefore = await menu.boundingBox();
  await card.hover();
  await expect(card.getByTitle("👍 でリアクション", { exact: true })).toBeVisible();
  expect(await menu.boundingBox()).toEqual(menuBoxBefore);

  await card.getByTitle("👍 でリアクション", { exact: true }).click();
  await expect(card.getByRole("button", { name: /👍\s*1/ })).toBeVisible({ timeout: 15_000 });
  await expect(card.getByTitle("👍 のリアクションを取り消す", { exact: true })).toHaveClass(/actionBtnActive/);

  // 青枠でないボタンは、付けているリアクションを外してその絵文字で付け直す。
  await card.getByTitle("🎉 でリアクション", { exact: true }).click();
  await expect(card.getByRole("button", { name: /🎉\s*1/ })).toBeVisible({ timeout: 15_000 });
  await expect(card.getByRole("button", { name: /👍\s*\d/ })).toHaveCount(0);
  await expect(card.getByTitle("🎉 のリアクションを取り消す", { exact: true })).toHaveClass(/actionBtnActive/);
  await expect(card.getByTitle("👍 でリアクション", { exact: true })).not.toHaveClass(/actionBtnActive/);
});

test("幅が足りないとクイックリアクションボタンを末尾から隠し、メニューボタンは残す", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2equicknarrow");
  const prefix = `クイック幅 ${Date.now()}`;
  await seedFrequentReactions(request, user.token, prefix);
  const text = `${prefix} 対象`;
  await createNote(request, user.token, text);

  await page.setViewportSize({ width: 360, height: 800 });
  await seedAuth(page, user.token);
  await page.goto("/");
  const card = page.locator("article").filter({ hasText: text }).first();
  await expect(card).toBeVisible({ timeout: 15_000 });

  const menu = card.getByRole("button", { name: "⋯" });
  await expect(menu).toBeVisible();
  await card.hover();
  await expect(card.getByTitle("😂 でリアクション", { exact: true })).toBeHidden({ timeout: 15_000 });
  const cardBox = await card.boundingBox();
  const menuBox = await menu.boundingBox();
  expect(cardBox && menuBox).toBeTruthy();
  expect(menuBox!.x + menuBox!.width).toBeLessThanOrEqual(cardBox!.x + cardBox!.width);

  // 広げれば全部並ぶ。
  await page.setViewportSize({ width: 1600, height: 900 });
  await card.hover();
  await expect(card.getByTitle("😂 でリアクション", { exact: true })).toBeVisible({ timeout: 15_000 });
  await expect(menu).toBeVisible();
});

test.describe("タッチ環境", () => {
  test.use({ viewport: { width: 412, height: 900 }, hasTouch: true, isMobile: true });

  /** CDP でタッチを送る（Playwright の touchscreen は tap しかなく、指を滑らせられないため）。 */
  async function touch(cdp: CDPSession, type: "touchStart" | "touchMove" | "touchEnd", x: number, y: number) {
    await cdp.send("Input.dispatchTouchEvent", {
      type,
      touchPoints: type === "touchEnd" ? [] : [{ x, y }],
    });
  }

  test("❤️に指を置くとよく使う絵文字が左・上・右に出て、滑らせた方向の絵文字が付く", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2equickflick");
    const prefix = `クイックフリック ${Date.now()}`;
    await seedFrequentReactions(request, user.token, prefix);
    const text = `${prefix} 対象`;
    await createNote(request, user.token, text);

    await seedAuth(page, user.token);
    await page.goto("/");
    const card = page.locator("article").filter({ hasText: text }).first();
    await expect(card).toBeVisible({ timeout: 15_000 });

    // タッチ環境ではよく使う絵文字のボタンを行に並べない。
    const heart = card.getByTitle("❤️ でリアクション", { exact: true });
    await expect(heart).toBeVisible();
    await expect(card.getByTitle("👍 でリアクション", { exact: true })).toBeHidden({ timeout: 15_000 });

    const cdp = await page.context().newCDPSession(page);
    const heartButton = card.locator('button[title^="❤️ "]');
    const flickItems = card.locator('[class*="flickItem"]');

    /** ❤️に指を置き、`path` の順に滑らせて離す（各点は❤️の中心からの相対位置）。
     * リアクションのチップ行が出入りするとボタンの位置が変わるので、毎回測り直す。 */
    async function flick(path: [number, number][], onDown?: () => Promise<void>) {
      await expect(heartButton).toBeEnabled();
      const box = (await heartButton.boundingBox())!;
      const x = box.x + box.width / 2;
      const y = box.y + box.height / 2;
      await touch(cdp, "touchStart", x, y);
      await onDown?.();
      let last: [number, number] = [0, 0];
      for (const p of path) {
        await touch(cdp, "touchMove", x + p[0], y + p[1]);
        last = p;
      }
      await touch(cdp, "touchEnd", x + last[0], y + last[1]);
    }

    // 指を置いた時点でせり出す（左=👍、上=🎉、右=😂）。上へ滑らせて離すと🎉。
    await flick([[2, -20], [4, -45]], () => expect(flickItems).toHaveCount(3));
    await expect(flickItems).toHaveCount(0);
    await expect(card.getByRole("button", { name: /🎉\s*1/ })).toBeVisible({ timeout: 15_000 });

    // 左へ滑らせると👍に付け替わる。
    await flick([[-20, 0], [-45, 3]]);
    await expect(card.getByRole("button", { name: /👍\s*1/ })).toBeVisible({ timeout: 15_000 });
    await expect(card.getByRole("button", { name: /🎉\s*\d/ })).toHaveCount(0);

    // 下へ滑らせると何もしない。
    await flick([[0, 20], [0, 45]]);
    await expect(flickItems).toHaveCount(0);
    await expect(card.getByRole("button", { name: /👍\s*1/ })).toBeVisible();

    // 動かさずに離すと❤️。
    await flick([]);
    await expect(card.getByRole("button", { name: /❤️\s*1/ })).toBeVisible({ timeout: 15_000 });
    await expect(card.getByRole("button", { name: /👍\s*\d/ })).toHaveCount(0);
    await expect(card.getByTitle("❤️ のリアクションを取り消す", { exact: true })).toHaveClass(/actionBtnActive/);
  });
});
