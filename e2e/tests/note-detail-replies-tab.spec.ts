import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// ポスト詳細画面の返信セクション（#226）: 対象ポストへの直系リプライ・引用を
// 再帰的にツリー表示する。3ペイン表示（既定のe2eビューポート幅）では中央ペイン
// 下部の常設セクションとして表示される（#241）。
test("ポスト詳細の返信タブに返信・引用・孫リプライが再帰的に表示される", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2erepliestabauth");
  const replier = await registerUserViaApi(request, "e2erepliestabusr");

  const rootText = `根ポスト ${Date.now()}`;
  const rootRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: rootText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
  });
  expect(rootRes.ok(), `create failed: ${rootRes.status()} ${await rootRes.text()}`).toBeTruthy();
  const root = await rootRes.json();

  const replyText = `直接返信 ${Date.now()}`;
  const replyRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${replier.token}` },
    data: {
      text: replyText,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
      visibility: "public",
      reply_to_id: root.id,
    },
  });
  expect(replyRes.ok(), `reply failed: ${replyRes.status()} ${await replyRes.text()}`).toBeTruthy();
  const reply = await replyRes.json();

  const quoteText = `引用テスト ${Date.now()}`;
  const quoteRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${replier.token}` },
    data: {
      text: quoteText,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
      visibility: "public",
      quote_of_id: root.id,
    },
  });
  expect(quoteRes.ok(), `quote failed: ${quoteRes.status()} ${await quoteRes.text()}`).toBeTruthy();

  const grandchildText = `孫リプライ ${Date.now()}`;
  const grandchildRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: {
      text: grandchildText,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
      visibility: "public",
      reply_to_id: reply.id,
    },
  });
  expect(
    grandchildRes.ok(),
    `grandchild reply failed: ${grandchildRes.status()} ${await grandchildRes.text()}`,
  ).toBeTruthy();

  await seedAuth(page, author.token);
  await page.goto(`/notes/${root.id}`);
  // 返信タブが中央ペインへ自動展開されるため、引用ポスト（quoteText）内の引用元カードにも
  // rootTextが埋め込まれ表示されうる。ここでは「ページが読み込めたか」の確認が目的なので
  // 最初の1件で十分（strict modeの複数一致エラー回避）。
  await expect(page.getByText(rootText).first()).toBeVisible({ timeout: 15_000 });

  // 返信タブは3ペイン表示（既定のe2eビューポート幅）では右ペインのタブから外れ、
  // 中央ペイン（<main>）下部の常設セクションへ自動表示される（クリック操作不要、#241）。
  // 中央ペインに絞ることで、右ペイン側の他タブの内容と混同しないようにする。
  const centerPane = page.getByRole("main");
  await expect(centerPane.getByText(replyText)).toBeVisible({ timeout: 15_000 });
  await expect(centerPane.getByText(quoteText)).toBeVisible({ timeout: 15_000 });
  await expect(centerPane.getByText(grandchildText)).toBeVisible({ timeout: 15_000 });
});
