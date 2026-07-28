import { test, expect } from "@playwright/test";
import { loginViaApi, registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { startStubS3Server } from "../fixtures/stub-s3-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e";
const MINIMAL_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC",
  "base64",
);

// Fedi配送は「自分のacceptedフォロワー全員へ配送」が基本の仕組みで、通常投稿・返信・
// リポストいずれも同じフォロワーファンアウト経路を通る
// (crates/seiran-common/src/ap/deliver.rs の fetch_fedi_follower_inboxes/fan_out_activity)。
// そのためスタブアクターに事前にフォローさせておけば、配送先inboxで受信内容を検証できる。
// なお通常投稿は本文中のメンション先もフォロー関係と無関係に配送先へ追加される
// （fetch_inboxes_by_ap_uris）が、e2eの stub-fedi-server はホストにポート番号を含むため
// （`@user@host:port`）本文メンションのドメインパーサーでは表現できず、この経路のE2E化は
// 別途 crates/seiran-common/src/ap/deliver.rs の unit test 側で担保している。

async function followAndWaitAccepted(fedi: StubFediServer, username: string) {
  await fedi.sendFollow(SEIRAN_BASE_URL, username);
  await expect
    .poll(() => fedi.receivedActivities().some((a) => a.type === "Accept"), { timeout: 15_000 })
    .toBeTruthy();
}

test.describe("Fedi配送", () => {
  let fedi: StubFediServer;

  test.beforeEach(async () => {
    fedi = await startStubFediServer();
  });

  test.afterEach(async () => {
    await fedi.close();
  });

  test("投稿がFediフォロワーへ配送される", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2afedipost");
    await followAndWaitAccepted(fedi, user.username);

    await seedAuth(page, user.token);
    await page.goto("/");
    const text = `Fedi配送テスト ${Date.now()}`;
    await page.getByPlaceholder("いまどうしてる？").fill(text);
    await page.getByRole("button", { name: "投稿", exact: true }).click();
    await expect(page.getByText(text)).toBeVisible({ timeout: 15_000 });

    await expect
      .poll(
        () => fedi.receivedActivities().some((a) => a.type === "Create" && (a.object as any)?.content?.includes(text)),
        { timeout: 15_000 },
      )
      .toBeTruthy();
  });

  test("本文カスタム絵文字がEmoji tag付きでFediへ配送される（#126）", async ({ request }) => {
    const s3 = await startStubS3Server();
    let providerId: string | null = null;
    let adminToken: string | null = null;
    try {
      const user = await registerUserViaApi(request, "e2afediemoji");
      adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
      await followAndWaitAccepted(fedi, user.username);

      const providerRes = await request.post("/api/admin/storage-providers", {
        headers: { Authorization: `Bearer ${adminToken}` },
        data: {
          name: `e2e-fedi-emoji-${Date.now()}`,
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
        multipart: {
          file: { name: "emoji.png", mimeType: "image/png", buffer: MINIMAL_PNG },
          media_type: "emoji",
        },
      });
      expect(uploadRes.ok(), await uploadRes.text()).toBeTruthy();
      const uploaded = await uploadRes.json();
      const shortcode = `fedidelivery${Date.now().toString(36)}`;
      const emojiRes = await request.post("/api/admin/emojis", {
        headers: { Authorization: `Bearer ${adminToken}` },
        data: { shortcode, media_file_id: uploaded.id },
      });
      expect(emojiRes.ok(), await emojiRes.text()).toBeTruthy();

      const text = `Fedi絵文字配送 :${shortcode}:`;
      const createRes = await request.post("/api/notes/create", {
        headers: { Authorization: `Bearer ${user.token}` },
        data: { text, deliver_to_fedi: true, deliver_to_bsky: false, visibility: "public" },
      });
      expect(createRes.ok(), await createRes.text()).toBeTruthy();

      await expect
        .poll(
          () => fedi.receivedActivities().find((a) => a.type === "Create" && (a.object as any)?.content?.includes(text)),
          { timeout: 15_000 },
        )
        .toBeTruthy();
      const activity = fedi
        .receivedActivities()
        .find((a) => a.type === "Create" && (a.object as any)?.content?.includes(text)) as any;
      const emojiTag = activity.object.tag.find((tag: any) => tag.type === "Emoji" && tag.name === `:${shortcode}:`);
      expect(emojiTag?.icon?.url).toContain(`${s3.url}/e2e-test/`);

      // Misskey系などが Create の object.id を再取得する経路でも Emoji tag が
      // 消えないことを確認する。配送JSONだけの検査では実機の不具合を検出できない。
      const canonicalPath = new URL(activity.object.id).pathname;
      const canonicalRes = await request.get(`${SEIRAN_BASE_URL}${canonicalPath}`, {
        headers: { Accept: "application/activity+json" },
      });
      expect(canonicalRes.ok(), await canonicalRes.text()).toBeTruthy();
      const canonicalNote = await canonicalRes.json();
      const canonicalEmojiTag = canonicalNote.tag.find(
        (tag: any) => tag.type === "Emoji" && tag.name === `:${shortcode}:`,
      );
      expect(canonicalEmojiTag?.icon?.url).toBe(emojiTag.icon.url);
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

  test("返信がFediフォロワーへinReplyTo付きで配送される", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2afedireply");
    await followAndWaitAccepted(fedi, user.username);

    const originalText = `元投稿 ${Date.now()}`;
    const createRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { text: originalText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
    });
    expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
    const original = await createRes.json();

    await seedAuth(page, user.token);
    await page.goto(`/notes/${original.id}`);
    await page.getByRole("button", { name: "返信" }).click();
    const replyText = `返信配送テスト ${Date.now()}`;
    await page.getByPlaceholder("返信を入力").fill(replyText);
    await page.getByRole("button", { name: "投稿", exact: true }).click();
    await expect(page.getByPlaceholder("返信を入力")).toHaveCount(0, { timeout: 15_000 });

    await expect
      .poll(
        () =>
          fedi
            .receivedActivities()
            .find((a) => a.type === "Create" && (a.object as any)?.content?.includes(replyText)),
        { timeout: 15_000 },
      )
      .toBeTruthy();

    const activity = fedi
      .receivedActivities()
      .find((a) => a.type === "Create" && (a.object as any)?.content?.includes(replyText)) as any;
    expect(activity.object.inReplyTo).toBeTruthy();
  });

  test("リポストがFediフォロワーへAnnounceとして配送される", async ({ page, request }) => {
    const user = await registerUserViaApi(request, "e2afedirepost");
    await followAndWaitAccepted(fedi, user.username);

    const originalText = `リポスト対象 ${Date.now()}`;
    const createRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { text: originalText, deliver_to_fedi: false, deliver_to_bsky: false, visibility: "public" },
    });
    expect(createRes.ok(), `create failed: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
    const original = await createRes.json();
    const expectedObjectId = `https://localhost/notes/${original.id}`;

    await seedAuth(page, user.token);
    await page.goto(`/notes/${original.id}`);
    await page.getByRole("button", { name: "リポスト" }).click();

    await expect
      .poll(
        () => fedi.receivedActivities().some((a) => a.type === "Announce" && a.object === expectedObjectId),
        { timeout: 15_000 },
      )
      .toBeTruthy();
  });
});
