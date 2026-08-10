import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

// ポスト詳細画面の右ペイン「返信」タブ（#226）: 対象ポストへの直系リプライ・引用を
// 再帰的にツリー表示する。
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
  await expect(page.getByText(rootText)).toBeVisible({ timeout: 15_000 });

  // NoteCard自体にも同名（title="返信"）の返信アクションボタンがあるため、
  // タブ切り替え時点ではまだそれらが描画されていない右ペイン（aside）内に絞って特定する。
  // 「前後のポスト」も自動読み込みされ中央ペインの狭幅表示に同じ本文が出うるため、
  // 以降のアサーションも右ペインに絞る。
  const rightPane = page.getByRole("complementary");
  await rightPane.getByRole("button", { name: "返信", exact: true }).click();
  await expect(rightPane.getByText(replyText)).toBeVisible({ timeout: 15_000 });
  await expect(rightPane.getByText(quoteText)).toBeVisible({ timeout: 15_000 });
  await expect(rightPane.getByText(grandchildText)).toBeVisible({ timeout: 15_000 });
});
