import { test, expect, APIRequestContext } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// BskyポストをFedi方面へリポストすると、Fedi側からはリポストラッパーが通常のノートに見え、
// リアクション（Like）がラッパー宛てに届く。ラッパー越しに元ポストを表示するときは
// 元ポスト宛て・ラッパー宛てのリアクションを合算して表示する。ここではラッパー宛ての
// リアクションをAPIで直接付けて再現する。

async function react(request: APIRequestContext, token: string, noteId: string, content: string) {
  const res = await request.post(`/api/notes/${noteId}/reactions`, {
    headers: { Authorization: `Bearer ${token}` },
    data: { content },
  });
  expect(res.ok(), `react failed: ${res.status()} ${await res.text()}`).toBeTruthy();
}

async function setup(request: APIRequestContext, prefix: string) {
  const author = await registerUserViaApi(request, `${prefix}a`);
  const reposter = await registerUserViaApi(request, `${prefix}r`);
  const viaWrapper = await registerUserViaApi(request, `${prefix}w`);
  const viaOriginal = await registerUserViaApi(request, `${prefix}o`);

  const text = `ラッパー宛てリアクション合算 ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text },
  });
  expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const original = await createRes.json();

  const repostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${reposter.token}` },
    data: { renote_id: original.id },
  });
  expect(repostRes.ok(), `repost failed: ${repostRes.status()} ${await repostRes.text()}`).toBeTruthy();
  const wrapper = await repostRes.json();

  await react(request, viaOriginal.token, original.id, "👍");
  await react(request, viaWrapper.token, wrapper.id, "👍");
  await react(request, viaWrapper.token, wrapper.id, "🎉");
  // 1投稿1リアクションまでなので、上の👍は🎉に切り替わる。別アクターで👍をもう1つ付ける。
  const viaWrapper2 = await registerUserViaApi(request, `${prefix}x`);
  await react(request, viaWrapper2.token, wrapper.id, "👍");

  return { author, reposter, viaWrapper, viaWrapper2, viaOriginal, text, original, wrapper };
}

test("リポストラッパーの詳細表示で元ポストとラッパーのリアクションが合算される", async ({ page, request }) => {
  const s = await setup(request, "e2ewrapreact");

  await seedAuth(page, s.reposter.token);
  await page.goto(`/notes/${s.wrapper.id}`);
  const card = page.locator("article").filter({ hasText: s.text }).first();
  await expect(card).toBeVisible({ timeout: 15_000 });
  await expect(card.getByRole("button", { name: /👍\s*2/ })).toBeVisible({ timeout: 15_000 });
  await expect(card.getByRole("button", { name: /🎉\s*1/ })).toBeVisible();

  // 右ペインのリアクションタブも、元ポスト・ラッパー双方のリアクターを並べる。
  const rightPane = page.getByRole("complementary");
  await rightPane.getByRole("button", { name: "リアクション", exact: true }).click();
  await expect(rightPane.getByText(s.viaOriginal.username)).toBeVisible({ timeout: 15_000 });
  await expect(rightPane.getByText(s.viaWrapper2.username)).toBeVisible();
  await expect(rightPane.getByText(s.viaWrapper.username)).toBeVisible();

  // 元ポストを直接表示したときは元ポスト宛ての分だけ。
  await page.goto(`/notes/${s.original.id}`);
  const originalCard = page.locator("article").filter({ hasText: s.text }).first();
  await expect(originalCard.getByRole("button", { name: /👍\s*1/ })).toBeVisible({ timeout: 15_000 });
  await expect(originalCard.getByRole("button", { name: /🎉/ })).toHaveCount(0);
});

test("Misskey互換API: 純粋リノートのrenoteにラッパー宛てのリアクションが合算される", async ({ request }) => {
  const s = await setup(request, "e2emkwrapreact");

  const showRes = await request.post("/api/notes/show", {
    headers: { Authorization: `Bearer ${s.viaOriginal.token}` },
    data: { noteId: s.wrapper.id },
  });
  expect(showRes.ok(), `notes/show failed: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.renote.reactions).toEqual({ "👍": 2, "🎉": 1 });
  // myReaction はリノート先（元ポスト）の自分のリアクションのまま。
  expect(shown.renote.myReaction).toBe("👍");

  const originalRes = await request.post("/api/notes/show", {
    headers: { Authorization: `Bearer ${s.viaOriginal.token}` },
    data: { noteId: s.original.id },
  });
  expect((await originalRes.json()).reactions).toEqual({ "👍": 1 });
});
