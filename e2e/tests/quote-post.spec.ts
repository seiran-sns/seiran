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
