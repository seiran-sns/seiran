import { test, expect } from "@playwright/test";
import { loginViaApi, registerUserViaApi } from "../fixtures/api-helpers";
import { startStubS3Server } from "../fixtures/stub-s3-server";

const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e";

// 1x1 透明PNG。
const MINIMAL_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
  "base64",
);

test("Misskey互換API: リポストのnotes/showでrenoteに元ノート本体が埋め込まれる（#74）", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2emkrenotea");
  const bob = await registerUserViaApi(request, "e2emkrenoteb");

  const originalText = `renote元ポスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${alice.token}` },
    data: { text: originalText },
  });
  expect(createRes.ok(), `元投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();
  const original = await createRes.json();

  const repostRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { renote_id: original.id },
  });
  expect(repostRes.ok(), `リポスト作成失敗: ${repostRes.status()} ${await repostRes.text()}`).toBeTruthy();
  const repost = await repostRes.json();

  const showRes = await request.post("/api/notes/show", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { noteId: repost.id },
  });
  expect(showRes.ok(), `notes/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.renoteId).toBe(String(original.id));
  expect(shown.renote, "renote本体がnullのまま（削除されたノート表示の原因）").not.toBeNull();
  expect(shown.renote.id).toBe(String(original.id));
  expect(shown.renote.text).toBe(originalText);
  expect(shown.renote.user.username).toBe(alice.username);
});

test("Misskey互換API: users/showのfollowersVisibility/followingVisibilityが常にpublic（#74）", async ({ request }) => {
  const alice = await registerUserViaApi(request, "e2emkvisa");
  const bob = await registerUserViaApi(request, "e2emkvisb");

  const showRes = await request.post("/api/users/show", {
    headers: { Authorization: `Bearer ${bob.token}` },
    data: { userId: alice.actorId },
  });
  expect(showRes.ok(), `users/show失敗: ${showRes.status()} ${await showRes.text()}`).toBeTruthy();
  const shown = await showRes.json();

  expect(shown.followersVisibility).toBe("public");
  expect(shown.followingVisibility).toBe("public");
  expect(typeof shown.followersCount).toBe("number");
  expect(typeof shown.followingCount).toBe("number");
});

test("Misskey互換API: i/notificationsのリアクション通知でローカルユーザーのavatarUrlが解決される（#74）", async ({
  request,
}) => {
  const s3 = await startStubS3Server();
  try {
    const alice = await registerUserViaApi(request, "e2emknotifa");
    const bob = await registerUserViaApi(request, "e2emknotifb");
    const adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);

    const providerRes = await request.post("/api/admin/storage-providers", {
      headers: { Authorization: `Bearer ${adminToken}` },
      data: {
        name: `e2e-stub-${Date.now()}`,
        endpoint: s3.url,
        bucket: "e2e-test",
        access_key: "stub",
        secret_key: "stub",
        public_url: `${s3.url}/e2e-test`,
      },
    });
    expect(providerRes.ok(), `ストレージプロバイダー登録失敗: ${providerRes.status()} ${await providerRes.text()}`).toBeTruthy();

    const uploadRes = await request.post("/api/drive/files/create", {
      headers: { Authorization: `Bearer ${bob.token}` },
      multipart: { file: { name: "avatar.png", mimeType: "image/png", buffer: MINIMAL_PNG }, media_type: "avatar" },
    });
    expect(uploadRes.ok(), `アバターアップロード失敗: ${uploadRes.status()} ${await uploadRes.text()}`).toBeTruthy();
    const uploaded = await uploadRes.json();

    const profileRes = await request.patch("/api/users/profile", {
      headers: { Authorization: `Bearer ${bob.token}` },
      data: { avatar_media_id: uploaded.id },
    });
    expect(profileRes.ok(), `プロフィール更新失敗: ${profileRes.status()} ${await profileRes.text()}`).toBeTruthy();

    const createRes = await request.post("/api/notes/create", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: { text: `通知アイコンテスト ${Date.now()}` },
    });
    expect(createRes.ok()).toBeTruthy();
    const created = await createRes.json();

    const reactRes = await request.post(`/api/notes/${created.id}/reactions`, {
      headers: { Authorization: `Bearer ${bob.token}` },
      data: { content: "🎉" },
    });
    expect(reactRes.ok(), `リアクション失敗: ${reactRes.status()} ${await reactRes.text()}`).toBeTruthy();

    const notifRes = await request.post("/api/i/notifications", {
      headers: { Authorization: `Bearer ${alice.token}` },
      data: {},
    });
    expect(notifRes.ok(), `i/notifications失敗: ${notifRes.status()} ${await notifRes.text()}`).toBeTruthy();
    const notifications = await notifRes.json();

    const reactionNotif = notifications.find((n: { type: string }) => n.type === "reaction");
    expect(reactionNotif, "リアクション通知が見つからない").toBeTruthy();
    expect(reactionNotif.user.avatarUrl, "ローカルユーザーのavatarUrlが解決されていない").not.toBeNull();
    expect(reactionNotif.user.avatarUrl).toContain(s3.url);
  } finally {
    await s3.close();
  }
});
