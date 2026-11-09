import { test, expect, type APIRequestContext } from "@playwright/test";
import { loginViaApi, registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

// 拒否サーバー（管理画面「サーバーブロック」）。スタブFediサーバーのホスト（127.0.0.1）を
// 拒否するので、他のスタブ利用specと並走しない globals-serial フェーズで実行する
// （playwright.config.ts の GLOBALS_SERIAL_SPECS）。

// global-setup.ts が作成する初期管理者アカウント（role=admin）。
const ADMIN_USERNAME = "e2ebootstrap";
const ADMIN_PASSWORD = "seiranda-e2e-admin-bootstrap";

async function putDeniedHosts(request: APIRequestContext, token: string, text: string): Promise<string[]> {
  const res = await request.put("/api/admin/denied-hosts", {
    headers: { Authorization: `Bearer ${token}` },
    data: { text },
  });
  expect(res.ok(), `denied-hosts PUT failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  return ((await res.json()) as { hosts: string[] }).hosts;
}

async function notificationTypes(request: APIRequestContext, token: string): Promise<string[]> {
  const res = await request.post("/api/i/notifications", {
    headers: { Authorization: `Bearer ${token}` },
    data: { limit: 20 },
  });
  if (!res.ok()) return [];
  return ((await res.json()) as { type: string }[]).map((n) => n.type);
}

test.describe("拒否サーバー", () => {
  let adminToken: string;
  let fedi: StubFediServer;

  test.beforeAll(async ({ request }) => {
    adminToken = await loginViaApi(request, ADMIN_USERNAME, ADMIN_PASSWORD);
  });

  test.beforeEach(async () => {
    fedi = await startStubFediServer();
  });

  test.afterEach(async ({ request }) => {
    await fedi.close();
    await putDeniedHosts(request, adminToken, "");
  });

  test("管理画面のtextareaで保存すると正規化された一覧に置き換わる", async ({ page }) => {
    await seedAuth(page, adminToken);
    await page.goto("/admin/server-blocks");
    const textarea = page.getByTestId("denied-hosts-textarea");
    await expect(textarea).toBeVisible({ timeout: 15_000 });

    await textarea.fill("https://Spam.Example/users/a\n\n*.foo.test\n@user@bar.test\nspam.example");
    await page.getByRole("button", { name: "保存" }).click();

    await expect(textarea).toHaveValue("bar.test\nfoo.test\nspam.example", { timeout: 15_000 });
    await page.reload();
    await expect(page.getByTestId("denied-hosts-textarea")).toHaveValue("bar.test\nfoo.test\nspam.example", {
      timeout: 15_000,
    });
  });

  test("フォローしていないローカルユーザーへのフォローはRejectされ、通知も届かない", async ({ request }) => {
    await putDeniedHosts(request, adminToken, new URL(fedi.actorUri).hostname);
    const alice = await registerUserViaApi(request, "e2edenyfol");

    await fedi.sendFollow(SEIRAN_BASE_URL, alice.username);

    await expect
      .poll(() => fedi.receivedActivities().some((a) => a.type === "Reject"), { timeout: 15_000 })
      .toBeTruthy();
    expect(fedi.receivedActivities().some((a) => a.type === "Accept")).toBeFalsy();
    expect(await notificationTypes(request, alice.token)).not.toContain("follow");
  });

  test("メンションはフォローしている本人にだけ届く", async ({ request }) => {
    await putDeniedHosts(request, adminToken, new URL(fedi.actorUri).hostname);
    const alice = await registerUserViaApi(request, "e2edenymna");
    const carol = await registerUserViaApi(request, "e2edenymnc");

    // carol はスタブ（鍵アカウント）をフォローし、Accept を受けて承認済みになる。
    const followRes = await request.post("/api/follows/create", {
      headers: { Authorization: `Bearer ${carol.token}` },
      data: { target: fedi.actorUri },
    });
    expect(followRes.ok(), `follow failed: ${followRes.status()} ${await followRes.text()}`).toBeTruthy();
    const followActivity = fedi.receivedActivities().find((a) => a.type === "Follow");
    expect(followActivity, "スタブがFollowを受信していない").toBeTruthy();
    await fedi.sendAccept(SEIRAN_BASE_URL, followActivity!);

    // alice（スタブをフォローしていない）宛てを先に送り、carol 宛ての到着を待ってから
    // alice に届いていないことを確かめる。
    await fedi.sendCreateNote(SEIRAN_BASE_URL, alice.username, `拒否メンション ${Date.now()}`, {
      mentionTargetUsername: alice.username,
      visibility: "public",
    });
    await expect
      .poll(
        async () => {
          await fedi.sendCreateNote(SEIRAN_BASE_URL, carol.username, `許可メンション ${Date.now()}`, {
            mentionTargetUsername: carol.username,
            visibility: "public",
          });
          return (await notificationTypes(request, carol.token)).includes("mention");
        },
        { timeout: 20_000, intervals: [2_000] },
      )
      .toBeTruthy();
    expect(await notificationTypes(request, alice.token)).not.toContain("mention");
  });
});
