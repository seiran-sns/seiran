import { expect, test } from "@playwright/test";
import { ADMIN_PASSWORD, ADMIN_USERNAME, loginViaApi, registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { uniquePng } from "../fixtures/png";
import { startStubS3Server } from "../fixtures/stub-s3-server";

// ライトボックスは閉じるとき history.back() して popstate で閉じる。ポスト詳細は popstate で
// React Router が描き直すので、そのときにも閉じること（閉じずに残ると、クリックのたびに
// history.back() して前の画面へ戻ってしまう）。
test("ポスト詳細で開いた画像のライトボックスは、背景のクリックで閉じてその画面に留まる", async ({
  page,
  request,
}) => {
  const s3 = await startStubS3Server();
  let providerId: string | null = null;
  let adminToken: string | null = null;
  try {
    const user = await registerUserViaApi(request, "e2elightbox");
    adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
    const providerRes = await request.post("/api/admin/storage-providers", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: {
        name: `e2e-lightbox-${Date.now()}`,
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
      headers: { Authorization: `Bearer ${user.token}` },
      multipart: { file: { name: "a.png", mimeType: "image/png", buffer: uniquePng() } },
    });
    expect(uploadRes.ok(), await uploadRes.text()).toBeTruthy();
    const file = await uploadRes.json();
    const noteRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { text: `ライトボックス ${Date.now()}`, attachment_ids: [String(file.id)] },
    });
    expect(noteRes.ok(), await noteRes.text()).toBeTruthy();
    const note = (await noteRes.json()) as { id: string };

    await seedAuth(page, user.token);
    await page.goto("/");
    await page.goto(`/notes/${note.id}`);
    const image = page.locator('main article [class*="attachments"] img').first();
    await image.click();
    const closeButton = page.getByRole("button", { name: "閉じる" });
    await expect(closeButton).toBeVisible({ timeout: 10_000 });

    // 背景（画像の外）をクリックすると閉じ、ポスト詳細のまま。
    const viewport = page.viewportSize();
    if (!viewport) throw new Error("no viewport");
    await page.mouse.click(viewport.width / 2, viewport.height - 10);
    await expect(closeButton).toHaveCount(0, { timeout: 5_000 });
    await expect(page).toHaveURL(new RegExp(`/notes/${note.id}`));
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
