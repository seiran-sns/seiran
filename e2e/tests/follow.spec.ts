import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";
import { startStubFediServer, type StubFediServer } from "../fixtures/stub-fedi-server";
import { BACKEND_URL as SEIRAN_BASE_URL } from "../ports.ts";

test("ローカルユーザーをフォローすると即座にフォロー中になる", async ({ page, request }) => {
  // ローカルユーザー同士のフォローは承認不要で即accepted
  // (crates/seiran-api/src/handlers/follows.rs)。
  const follower = await registerUserViaApi(request, "e2efol");
  const target = await registerUserViaApi(request, "e2etgt");
  await seedAuth(page, follower.token);

  await page.goto(`/@${target.username}`);
  // フォローボタンはユーザーメニュー内にも統合されているが、独立ボタンとしても
  // followArea に残しているため、メニューを開かず直接クリックできる。
  await page.getByRole("button", { name: "フォロー", exact: true }).click();

  // #56 でプロフィール右ペインに「フォロー中」タブが追加され、単純な getByText("フォロー中")
  // だとタブラベルと衝突するため、フォロー状態バッジ（span）に絞り込む。
  await expect(page.locator("span").filter({ hasText: "フォロー中" })).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("button", { name: "フォロー解除" })).toBeVisible();
});

test("プロフィール画面のフォロー中/フォロワータブに一覧が表示される（#56）", async ({ page, request }) => {
  const follower = await registerUserViaApi(request, "e2eflw");
  const target = await registerUserViaApi(request, "e2eflwtgt");

  const followRes = await request.post("/api/follows/create", {
    headers: { Authorization: `Bearer ${follower.token}` },
    data: { target: target.username },
  });
  expect(followRes.ok(), `follow failed: ${followRes.status()} ${await followRes.text()}`).toBeTruthy();

  // ターゲット側のプロフィール: フォロワータブに follower が表示される。
  await seedAuth(page, target.token);
  await page.goto(`/@${target.username}`);
  await page.getByText(/1\s*フォロワー|1\s*Followers?/).click();
  await expect(page.getByText(`@${follower.username}`)).toBeVisible({ timeout: 15_000 });

  // フォロワー側のプロフィール: フォロー中タブに target が表示される。
  await seedAuth(page, follower.token);
  await page.goto(`/@${follower.username}`);
  await page.getByText(/1\s*フォロー中|1\s*Following/).click();
  await expect(page.getByText(`@${target.username}`)).toBeVisible({ timeout: 15_000 });
});

test.describe("Fediフォロー承認のリアルタイム反映", () => {
  let fedi: StubFediServer;

  test.beforeEach(async () => {
    fedi = await startStubFediServer();
  });

  test.afterEach(async () => {
    await fedi.close();
  });

  test("承認待ちからAccept受信でプロフィール画面がリロードなしにフォロー中になる", async ({ page, request }) => {
    // Fediフォローは常にpendingで開始し、相手からのAcceptが非同期で届いて初めてacceptedになる
    // (crates/seiran-api/src/handlers/follows.rs の follow_fedi、
    //  crates/seiran-common/src/jobs/inbound_activity_process.rs の handle_accept)。
    // このテストは、Acceptの到着をWebSocketの `followAccepted` イベントで
    // 画面へリアルタイム反映する経路（frontend/src/pages/ProfilePage.tsx）を検証する。
    const user = await registerUserViaApi(request, "e2afedifol");

    const followRes = await request.post("/api/follows/create", {
      headers: { Authorization: `Bearer ${user.token}` },
      data: { target: fedi.actorUri },
    });
    expect(followRes.ok(), `follow failed: ${followRes.status()} ${await followRes.text()}`).toBeTruthy();
    expect((await followRes.json()).status).toBe("pending");

    const followActivity = fedi.receivedActivities().find((a) => a.type === "Follow");
    expect(followActivity, "スタブがFollowを受信していない").toBeTruthy();

    const remoteUsername = "e2efedibot";
    const remoteDomain = new URL(fedi.actorUri).host;

    await seedAuth(page, user.token);
    await page.goto(`/@${remoteUsername}@${remoteDomain}`);
    await expect(page.getByText("承認待ち")).toBeVisible({ timeout: 15_000 });

    // ここでAcceptを送る。ページはリロードせず、WebSocket経由のfollowAcceptedイベントだけで
    // 「フォロー中」に切り替わることを確認する。
    await fedi.sendAccept(SEIRAN_BASE_URL, followActivity!);

    // #56 でプロフィール右ペインに「フォロー中」タブが追加され、単純な getByText("フォロー中")
    // だとタブラベルと衝突するため、フォロー状態バッジ（span）に絞り込む。
    await expect(page.locator("span").filter({ hasText: "フォロー中" })).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText("承認待ち")).toHaveCount(0);
  });
});
