import { test, expect, APIRequestContext, CDPSession, Locator, Page } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// NoteCard のクイックリアクション: ボタンバーの❤️（1クリック）と、ボタンバーのすぐ下に出る
// よく使う絵文字のポップアップ（❤️を除く、横幅に入るだけ）。ポップアップはマウス環境では
// ホバー中のカード、タッチ環境では画面内でいちばん上のカードにだけ出る。

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
async function seedReactions(request: APIRequestContext, token: string, prefix: string, usage: string[]) {
  for (const [i, emoji] of usage.entries()) {
    const note = await createNote(request, token, `${prefix} 下準備 ${i}`);
    await react(request, token, note.id, emoji);
  }
}

// 👍3・🎉2・❤️2（❤️はボタンバーにあるのでポップアップには出ない）・🙏1・😂1（同数なら新しい方が上）。
const BASIC_USAGE = ["👍", "👍", "👍", "🎉", "🎉", "❤️", "❤️", "🙏", "😂"];

function popup(card: Locator) {
  return card.getByRole("toolbar");
}

function popupTitles(card: Locator) {
  return popup(card)
    .locator("button")
    .evaluateAll((els) => els.map((e) => e.getAttribute("title")));
}

async function openHome(page: Page, token: string, text: string) {
  await seedAuth(page, token);
  await page.goto("/");
  const card = page.locator("article").filter({ hasText: text }).first();
  await expect(card).toBeVisible({ timeout: 15_000 });
  return card;
}

test("❤️ボタンで付けて、青枠になったボタンで取り消せる", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2equickheart");
  const text = `クイックリアクションいいね ${Date.now()}`;
  await createNote(request, user.token, text);
  const card = await openHome(page, user.token, text);

  // まだ何も使っていないユーザーにはポップアップが無い。
  await card.hover();
  await expect(popup(card)).toHaveCount(0);

  await card.getByTitle("❤️ でリアクション", { exact: true }).click();
  await expect(card.getByRole("button", { name: /❤️\s*1/ })).toBeVisible({ timeout: 15_000 });
  const removeButton = card.getByTitle("❤️ のリアクションを取り消す", { exact: true });
  await expect(removeButton).toHaveClass(/actionBtnActive/);
  await expect(card.getByTitle("リアクションを付ける", { exact: true })).toHaveClass(/actionBtnActive/);

  await removeButton.click();
  await expect(card.getByRole("button", { name: /❤️\s*\d/ })).toHaveCount(0, { timeout: 15_000 });
  await expect(card.getByTitle("❤️ でリアクション", { exact: true })).not.toHaveClass(/actionBtnActive/);
});

test("ホバー中のカードにだけポップアップが出て、❤️を除くよく使う順に並び、付け替え・取り消しができる", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2equickpop");
  const prefix = `クイックポップ ${Date.now()}`;
  await seedReactions(request, user.token, prefix, BASIC_USAGE);
  const text = `${prefix} 対象`;
  await createNote(request, user.token, text);

  await page.setViewportSize({ width: 1600, height: 900 });
  const card = await openHome(page, user.token, text);
  const menu = card.getByRole("button", { name: "⋯" });

  await page.mouse.move(0, 0);
  await expect(popup(card)).toBeHidden();
  const menuBoxBefore = await menu.boundingBox();

  await card.hover();
  await expect(popup(card)).toBeVisible();
  // ポップアップは重ねて出すので、ボタンバーの並びは動かない。
  expect(await menu.boundingBox()).toEqual(menuBoxBefore);
  await expect
    .poll(() => popupTitles(card))
    .toEqual(["👍 でリアクション", "🎉 でリアクション", "😂 でリアクション", "🙏 でリアクション"]);
  // 他のカードには出ない。
  const otherCard = page.locator("article").filter({ hasText: `${prefix} 下準備 0` }).first();
  await expect(popup(otherCard)).toBeHidden();

  await popup(card).getByTitle("👍 でリアクション", { exact: true }).click();
  await expect(card.getByRole("button", { name: /👍\s*1/ })).toBeVisible({ timeout: 15_000 });
  await expect(popup(card).getByTitle("👍 のリアクションを取り消す", { exact: true })).toHaveClass(
    /itemActive/
  );

  // 青枠でない絵文字は、付けているリアクションを外してその絵文字で付け直す。
  await popup(card).getByTitle("🎉 でリアクション", { exact: true }).click();
  await expect(card.getByRole("button", { name: /🎉\s*1/ })).toBeVisible({ timeout: 15_000 });
  await expect(card.getByRole("button", { name: /👍\s*\d/ })).toHaveCount(0);

  // 青枠の絵文字は取り消し。
  await popup(card).getByTitle("🎉 のリアクションを取り消す", { exact: true }).click();
  await expect(card.getByRole("button", { name: /🎉\s*\d/ })).toHaveCount(0, { timeout: 15_000 });
});

test("ポップアップには横幅に入るだけ並べ、途中で切れた絵文字を出さない。メニューボタンは隠れない", async ({
  page,
  request,
}) => {
  const user = await registerUserViaApi(request, "e2equicknarrow");
  const prefix = `クイック幅 ${Date.now()}`;
  const many = ["👍", "🎉", "😂", "🙏", "👀", "🔥", "✨", "😭", "🤔", "😇", "🥳", "🙌", "💯", "🍣"];
  await seedReactions(request, user.token, prefix, many);
  const text = `${prefix} 対象`;
  await createNote(request, user.token, text);

  await page.setViewportSize({ width: 360, height: 800 });
  const card = await openHome(page, user.token, text);
  await card.hover();
  await expect(popup(card)).toBeVisible();

  const menuBox = (await card.getByRole("button", { name: "⋯" }).boundingBox())!;
  const cardBox = (await card.boundingBox())!;
  expect(menuBox.x + menuBox.width).toBeLessThanOrEqual(cardBox.x + cardBox.width);

  // 1行目に収まった絵文字は全体が見え、入りきらない分は2行目に回って見えない。
  const popupBox = (await popup(card).boundingBox())!;
  expect(popupBox.x + popupBox.width).toBeLessThanOrEqual(cardBox.x + cardBox.width);
  const items = await popup(card)
    .locator("button")
    .evaluateAll((els) =>
      els.map((e) => {
        const r = e.getBoundingClientRect();
        return { left: r.left, right: r.right, top: r.top };
      })
    );
  const firstRowTop = items[0].top;
  const shown = items.filter((r) => r.top === firstRowTop);
  expect(shown.length).toBeGreaterThan(0);
  expect(shown.length).toBeLessThan(many.length);
  for (const r of shown) expect(r.right).toBeLessThanOrEqual(popupBox.x + popupBox.width);
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

  test("画面内でいちばん上のカードにだけポップアップが出て、スクロールで移る。❤️の上から始めたスクロールでは付かない", async ({
    page,
    request,
  }) => {
    const user = await registerUserViaApi(request, "e2equicktouch");
    const prefix = `クイックタッチ ${Date.now()}`;
    await seedReactions(request, user.token, prefix, BASIC_USAGE);
    const olderText = `${prefix} 対象A`;
    await createNote(request, user.token, olderText);
    const newerText = `${prefix} 対象B`;
    await createNote(request, user.token, newerText);

    const top = await openHome(page, user.token, newerText);
    const next = page.locator("article").filter({ hasText: olderText }).first();
    await expect(top.getByTitle("❤️ でリアクション", { exact: true })).toBeVisible();

    // 新しい方（いちばん上）にだけ出る。
    await expect(popup(top)).toBeVisible({ timeout: 15_000 });
    await expect(popup(next)).toBeHidden();
    await popup(top).getByTitle("🎉 でリアクション", { exact: true }).tap();
    await expect(top.getByRole("button", { name: /🎉\s*1/ })).toBeVisible({ timeout: 15_000 });

    // 上のカードのボタンバーが画面外へ出ると、次のカードに移る。
    const topBox = (await top.boundingBox())!;
    await page.evaluate((y) => window.scrollBy(0, y), topBox.y + topBox.height);
    await expect(popup(next)).toBeVisible({ timeout: 15_000 });
    await expect(popup(top)).toBeHidden();

    // ❤️に指が乗った状態から始めたスクロールでは❤️は付かない。
    const heart = next.getByTitle("❤️ でリアクション", { exact: true });
    const box = (await heart.boundingBox())!;
    const x = box.x + box.width / 2;
    const y = box.y + box.height / 2;
    const cdp = await page.context().newCDPSession(page);
    await touch(cdp, "touchStart", x, y);
    await touch(cdp, "touchMove", x, y - 30);
    await touch(cdp, "touchMove", x, y - 80);
    await touch(cdp, "touchEnd", x, y - 80);
    await page.waitForTimeout(1000);
    await expect(next.getByRole("button", { name: /❤️\s*\d/ })).toHaveCount(0);
  });
});
