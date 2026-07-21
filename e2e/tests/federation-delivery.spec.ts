import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

// Fedi配送は「自分のacceptedフォロワー全員へ配送」という単一の仕組みで、通常投稿・返信・
// リポストいずれも同じフォロワーファンアウト経路を通る
// (crates/seiran-common/src/ap/deliver.rs の fetch_fedi_follower_inboxes/fan_out_activity)。
// そのためスタブアクターに事前にフォローさせておけば、配送先inboxで受信内容を検証できる。

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
