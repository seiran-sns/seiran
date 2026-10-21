import { expect, test } from "@playwright/test";
import type { APIRequestContext, Page } from "@playwright/test";
import { loginViaApi, registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubS3Server } from "../fixtures/stub-s3-server";

const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e-admin-bootstrap";

test.use({ viewport: { width: 1600, height: 900 } });

// 1x1の最小有効PNG（アップロードは実デコードするためダミーバイト列では通らない）。
const MINIMAL_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC",
  "base64",
);

const REDRAFT_ITEM = "✏️ 削除して編集再投稿";

async function createNote(
  request: APIRequestContext,
  token: string,
  data: Record<string, unknown>,
): Promise<string> {
  const res = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${token}` },
    data: { deliver_to_fedi: false, deliver_to_bsky: false, ...data },
  });
  expect(res.ok(), `投稿作成失敗: ${res.status()} ${await res.text()}`).toBeTruthy();
  return (await res.json()).id as string;
}

async function hiddenGuides(request: APIRequestContext, token: string): Promise<string[]> {
  const res = await request.get("/api/auth/me", { headers: { Authorization: `Bearer ${token}` } });
  return (await res.json()).hidden_guides as string[];
}

async function noteStatus(request: APIRequestContext, token: string, id: string): Promise<number> {
  const res = await request.get(`/api/notes/${id}`, { headers: { Authorization: `Bearer ${token}` } });
  return res.status();
}

async function openMenu(page: Page, text: string) {
  const card = page.locator("article", { hasText: text });
  const trigger = card.getByRole("button", { name: "⋯" });
  await expect(trigger).toBeVisible({ timeout: 15_000 });
  await trigger.click();
  return card;
}

test("削除して編集再投稿: 確認後にポストが削除され、元の内容でコンポーザが開く。今後は表示しないで確認を省略できる", async ({
  page,
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2eredraft");
  const text1 = `再投稿テスト1 ${Date.now()}`;
  const text2 = `再投稿テスト2 ${Date.now()}`;
  // CW付きの本文は折りたたまれてカードのテキストに現れないため、CW文言でカードを探す。
  const cw = `ネタバレ${Date.now()}`;
  const id1 = await createNote(request, alice.token, { text: text1, content_warning: cw });
  const id2 = await createNote(request, alice.token, { text: text2 });
  const text3 = `再投稿テスト3 ${Date.now()}`;
  const id3 = await createNote(request, alice.token, { text: text3 });

  await seedAuth(page, alice.token);
  await page.goto("/");

  // 1件目: 確認ダイアログで「今後は表示しない」を選んで実行する。
  let card = await openMenu(page, cw);
  await card.getByRole("button", { name: REDRAFT_ITEM }).click();
  await expect(page.getByText("削除して編集再投稿しますか？")).toBeVisible({ timeout: 5_000 });
  await page.getByLabel("今後は表示しない").check();
  await page.getByRole("button", { name: "削除して編集する" }).click();

  await expect(page.locator("article", { hasText: cw })).toHaveCount(0, { timeout: 10_000 });
  expect(await noteStatus(request, alice.token, id1)).toBe(404);
  // 削除と同時に開いたコンポーザに、本文とCWが入っている。
  const editor = page.locator('[contenteditable="true"]').last();
  await expect(editor).toContainText(text1);
  await expect(page.locator('input[class*="cwGuideInput"]').last()).toHaveValue(cw);
  await expect.poll(() => hiddenGuides(request, alice.token)).toContain("redraft_note");

  // 閉じて2件目: 1件目の下書きが未投稿のまま残っているので、ガイドを非表示にしていても
  // 「下書きを置き換える」確認だけは出る（作成中の内容が失われるため）。
  await page.keyboard.press("Escape");
  card = await openMenu(page, text2);
  await card.getByRole("button", { name: REDRAFT_ITEM }).click();
  await expect(page.getByText("この投稿先には作成中の下書きがあり、置き換えられます。")).toBeVisible({
    timeout: 5_000,
  });
  await expect(page.getByLabel("今後は表示しない")).toHaveCount(0);
  await page.getByRole("button", { name: "削除して編集する" }).click();
  await expect(page.locator("article", { hasText: text2 })).toHaveCount(0, { timeout: 10_000 });
  expect(await noteStatus(request, alice.token, id2)).toBe(404);
  await expect(page.locator('[contenteditable="true"]').last()).toContainText(text2);

  // 下書きを投稿して消化すれば、非表示にしたガイドは出ず、そのまま削除してコンポーザが開く。
  await page.getByRole("button", { name: "投稿", exact: true }).last().click();
  await expect(page.locator("article", { hasText: text2 })).toHaveCount(1, { timeout: 15_000 });
  card = await openMenu(page, text3);
  await card.getByRole("button", { name: REDRAFT_ITEM }).click();
  await expect(page.locator("article", { hasText: text3 })).toHaveCount(0, { timeout: 10_000 });
  expect(await noteStatus(request, alice.token, id3)).toBe(404);
  await expect(page.locator('[contenteditable="true"]').last()).toContainText(text3);

  // 設定画面で非表示を解除できる。
  await page.goto("/settings/guides");
  const checkbox = page.getByLabel("「削除して編集再投稿」の確認");
  await expect(checkbox).toBeChecked();
  // 保存の応答でチェック状態が変わる制御コンポーネントなので、uncheck()（クリック直後の状態を検証する）は使わない。
  await checkbox.click();
  await expect(checkbox).not.toBeChecked();
  await expect.poll(() => hiddenGuides(request, alice.token)).not.toContain("redraft_note");
});

test("削除して編集再投稿: 添付は引き継がれ、削除直後も使い回せる", async ({ page, request }) => {
  const s3 = await startStubS3Server();
  let providerId: string | null = null;
  let adminToken: string | null = null;
  try {
    const alice = await registerUserViaApi(request, "e2eredraftmedia");
    adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    const providerRes = await request.post("/api/admin/storage-providers", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: {
        name: `e2e-redraft-${Date.now()}`,
        endpoint: s3.url,
        bucket: "e2e-test",
        access_key: "stub",
        secret_key: "stub",
        public_url: `${s3.url}/e2e-test`,
      },
    });
    expect(providerRes.ok(), await providerRes.text()).toBeTruthy();
    providerId = (await providerRes.json()).id;

    const uploadRes = await request.post("/api/drive/files/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      multipart: { file: { name: "a.png", mimeType: "image/png", buffer: MINIMAL_PNG } },
    });
    expect(uploadRes.ok(), `アップロード失敗: ${uploadRes.status()} ${await uploadRes.text()}`).toBeTruthy();
    const file = await uploadRes.json();

    const text = `添付つき再投稿 ${Date.now()}`;
    await createNote(request, alice.token, { text, attachment_ids: [String(file.id)] });

    await seedAuth(page, alice.token);
    await page.goto("/");
    const card = await openMenu(page, text);
    await card.getByRole("button", { name: REDRAFT_ITEM }).click();
    await page.getByRole("button", { name: "削除して編集する" }).click();

    await expect(page.locator("article", { hasText: text })).toHaveCount(0, { timeout: 10_000 });
    await expect(page.locator('[contenteditable="true"]').last()).toContainText(text);
    await expect(page.locator('img[class*="attachThumb"]')).toHaveCount(1);

    // そのまま投稿し直すと、同じ添付が付いた新しいポストになる。
    await page.getByRole("button", { name: "投稿", exact: true }).last().click();
    await expect(page.locator("article", { hasText: text }).locator("img").first()).toBeVisible({
      timeout: 15_000,
    });
  } finally {
    if (providerId && adminToken) {
      await request.patch(`/api/admin/storage-providers/${providerId}`, {
        headers: { Authorization: `Bearer ${adminToken}` },
        data: { is_active: false },
      });
    }
    await s3.close();
  }
});

test("削除して編集再投稿: 返信先が既に削除されていると不活性になり理由が表示される", async ({
  page,
  request,
}) => {
  const alice = await registerUserViaApi(request, "e2eredraftgone");
  const parentId = await createNote(request, alice.token, { text: `親 ${Date.now()}` });
  const text = `返信 ${Date.now()}`;
  await createNote(request, alice.token, { text, reply_to_id: parentId });
  const delRes = await request.delete(`/api/notes/${parentId}`, {
    headers: { Authorization: `Bearer ${alice.token}` },
  });
  expect(delRes.ok()).toBeTruthy();

  await seedAuth(page, alice.token);
  await page.goto("/");
  const card = await openMenu(page, text);
  const item = card.getByRole("button", { name: REDRAFT_ITEM });
  await expect(item).toBeDisabled();
  await expect(item).toHaveAttribute("title", "返信先が存在しないため再投稿できません");
});
