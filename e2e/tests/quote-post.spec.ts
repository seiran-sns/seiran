import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("引用元をAPIで1段だけ埋め込み、本文下の引用カードに表示する", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2equote");
  const headers = { Authorization: `Bearer ${user.token}` };
  const originalText = `引用元本文 ${Date.now()}`;

  const originalRes = await request.post("/api/notes/create", {
    headers,
    data: { text: originalText, deliver_to_fedi: false, deliver_to_bsky: false },
  });
  expect(originalRes.ok(), await originalRes.text()).toBeTruthy();
  const original = (await originalRes.json()) as { id: string };

  const nestedRes = await request.post("/api/notes/create", {
    headers,
    data: {
      text: "引用の引用になる投稿",
      quote_of_id: original.id,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
    },
  });
  expect(nestedRes.ok(), await nestedRes.text()).toBeTruthy();
  const nested = (await nestedRes.json()) as { id: string };

  const quoteText = `引用ポストのテスト ${Date.now()}`;
  const quoteRes = await request.post("/api/notes/create", {
    headers,
    data: {
      text: quoteText,
      quote_of_id: nested.id,
      deliver_to_fedi: false,
      deliver_to_bsky: false,
    },
  });
  expect(quoteRes.ok(), await quoteRes.text()).toBeTruthy();
  const quote = (await quoteRes.json()) as {
    id: string;
    quoteId: string;
    quote: { id: string; text: string; quoteId: string; quote?: unknown };
  };
  expect(quote.quoteId).toBe(nested.id);
  expect(quote.quote.id).toBe(nested.id);
  expect(quote.quote.text).toBe("引用の引用になる投稿");
  expect(quote.quote.quoteId).toBe(original.id);
  expect(quote.quote.quote).toBeUndefined();

  await seedAuth(page, user.token);
  await page.goto(`/notes/${quote.id}`);
  const card = page.locator("article", { hasText: quoteText });
  await expect(card.getByText("引用の引用になる投稿")).toBeVisible();
  await expect(card.getByText("❝ 引用あり")).toBeVisible();
  await expect(card.getByText(originalText)).toHaveCount(0);
});

test("投稿カードの引用ボタンからコメント付き引用を作成できる", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2equoteauthor");
  const quoter = await registerUserViaApi(request, "e2equoter");
  const originalText = `UI引用元 ${Date.now()}`;
  const originalRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${author.token}` },
    data: { text: originalText, deliver_to_fedi: false, deliver_to_bsky: false },
  });
  expect(originalRes.ok(), await originalRes.text()).toBeTruthy();

  await seedAuth(page, quoter.token);
  await page.goto(`/@${author.username}`);
  const originalCard = page.locator("article", { hasText: originalText });
  await originalCard.getByRole("button", { name: "引用" }).click();

  await expect(page.getByText("引用ポスト", { exact: true })).toBeVisible();
  await expect(page.getByText(originalText, { exact: true }).last()).toBeVisible();
  const quoteText = `UIからの引用 ${Date.now()}`;
  await page.getByPlaceholder("コメントを入力").fill(quoteText);
  await page.getByRole("button", { name: "投稿", exact: true }).last().click();
  await expect(page.getByText("引用ポスト", { exact: true })).toHaveCount(0);

  const timelineRes = await request.get("/api/notes/home-timeline", {
    headers: { Authorization: `Bearer ${quoter.token}` },
  });
  expect(timelineRes.ok(), await timelineRes.text()).toBeTruthy();
  const notes = (await timelineRes.json()) as Array<{
    text: string;
    quoteId?: string;
    quote?: { text: string };
  }>;
  expect(notes.find((note) => note.text === quoteText)).toMatchObject({
    quote: { text: originalText },
  });
});
