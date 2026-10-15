import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("プロフィールのアクティビティを種別とメディア添付で絞り込める", async ({ page, request }) => {
  const alice = await registerUserViaApi(request, "e2epffalice");
  const bob = await registerUserViaApi(request, "e2epffbob");
  const stamp = Date.now();
  const post = async (token: string, data: Record<string, unknown>) => {
    const res = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${token}` },
      data,
    });
    expect(res.ok(), await res.text()).toBeTruthy();
    return res.json();
  };

  const bobNote = await post(bob.token, { text: `bobの投稿 ${stamp}` });
  const normalText = `aliceの通常投稿 ${stamp}`;
  const replyToBobText = `aliceからbobへの返信 ${stamp}`;
  await post(alice.token, { text: normalText });
  await post(alice.token, { text: replyToBobText, reply_to_id: bobNote.id });
  const react = await request.post(`/api/notes/${bobNote.id}/reactions`, {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { content: "🎉" },
  });
  expect(react.ok(), await react.text()).toBeTruthy();

  // bob から alice のプロフィールを見る（「あなたへの返信」は bob 宛ての返信）。
  await seedAuth(page, bob.token);
  await page.goto(`/@${alice.username}`);
  const normal = page.getByText(normalText);
  const replyToBob = page.getByText(replyToBobText);
  const reaction = page.getByText(/にリアクションしました/);
  await expect(normal).toBeVisible({ timeout: 15_000 });
  await expect(replyToBob).toBeVisible();
  await expect(reaction).toBeVisible();

  // 種別: 通常ポストだけ。
  await page.getByRole("button", { name: "種別で絞り込み" }).click();
  await page.getByLabel("通常ポスト", { exact: true }).check();
  await expect(replyToBob).toBeHidden();
  await expect(reaction).toBeHidden();
  await expect(normal).toBeVisible();
  // 「あなたへの返信」を足すと（OR）bob宛ての返信も出る。
  await page.getByLabel("あなたへの返信", { exact: true }).check();
  await expect(replyToBob).toBeVisible();
  await expect(normal).toBeVisible();
  await page.keyboard.press("Escape");

  // ✕で種別を全部外すとリアクションも戻る。
  await page.getByRole("button", { name: "すべてクリア" }).click();
  await expect(reaction).toBeVisible();

  // メディア添付: 「なし」を選ぶと他の選択肢が選べなくなり、逆も同じ。リアクションには影響しない。
  await page.getByRole("button", { name: "メディア添付で絞り込み" }).click();
  await page.getByLabel("なし", { exact: true }).check();
  await expect(page.getByLabel("画像", { exact: true })).toBeDisabled();
  await expect(page.getByLabel("動画", { exact: true })).toBeDisabled();
  await expect(page.getByLabel("音声", { exact: true })).toBeDisabled();
  await expect(normal).toBeVisible();
  await expect(reaction).toBeVisible();
  await page.getByLabel("なし", { exact: true }).uncheck();
  await page.getByLabel("画像", { exact: true }).check();
  await expect(page.getByLabel("なし", { exact: true })).toBeDisabled();
  // 画像つき投稿は無いので、投稿は消えるがリアクションは残る。
  await expect(normal).toBeHidden();
  await expect(reaction).toBeVisible();
});
