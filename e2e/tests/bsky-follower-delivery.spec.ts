import { test, expect } from "@playwright/test";
import { registerUserViaApi, seedAuth, getOwnDid } from "../fixtures/api-helpers";
import { registerBskyFollowers } from "../fixtures/stub-appview-control";
import { waitForBskyFeedPost } from "../fixtures/subscribe-repos-client";

// Bsky側の配送E2E（docs/roadmap.md）: リモートBskyアクターからのフォロー受理を
// `bsky_follower_poll`（`app.bsky.graph.getFollowers` ポーリング、間隔は playwright.config.ts の
// backendEnv.BSKY_FOLLOWER_POLL_INTERVAL_SECS=2 で短縮済み）経由で検知できること、
// および検知後の投稿が `subscribeRepos` で正しく配送されることを一気通貫で検証する。
const SEIRAN_BASE_URL = "http://localhost:3000";
const APPVIEW_BASE_URL = `http://127.0.0.1:${process.env.APPVIEW_STUB_PORT ?? "2583"}`;

// NotificationsPanel（クイック通知）はHomePageの右ペインにあり、AppShellのレスポンシブCSSに
// より既定のテストビューポート（1280px）幅では非表示になる。右ペインが表示される幅を指定する。
test.use({ viewport: { width: 1600, height: 900 } });

test("リモートBskyフォロワーの検知と投稿のsubscribeRepos配送", async ({ page, request }) => {
  const user = await registerUserViaApi(request, "e2abskyfollow");
  const did = await getOwnDid(request, user.token, user.username);

  // 1. 空のフォロワーリストでbaselineを確立する（無通知のはず）。ポーリング間隔2秒に対して
  //    十分な余裕を持って待つ。
  await registerBskyFollowers(request, APPVIEW_BASE_URL, did, []);
  await page.waitForTimeout(4_000);

  // 2. 合成フォロワーを1件追加登録する（baseline確立後の「新規フォロー」として検知されるはず）。
  const followerHandle = `e2ebskyfollower${Date.now()}.bsky.social`;
  const followerDid = `did:plc:e2ebskyfollower${Date.now()}`;
  await registerBskyFollowers(request, APPVIEW_BASE_URL, did, [
    { did: followerDid, handle: followerHandle },
  ]);

  // 3. フォロー通知がクイック通知に表示されることを確認する（フォロー検知の検証）。
  //    label はハンドルからどう組み立てられるか NotificationsPanel.tsx を確認済み:
  //    Bskyアクターの domain は '' で local_domain と不一致のため host は Some('')（空文字）
  //    になり、`n.user?.host` が偽値扱いになるため handle 部分（@user@host）は付与されず、
  //    label は `who`（display_name が無い場合は username=ハンドル）のみになる。
  await seedAuth(page, user.token);
  await page.goto("/");
  await expect(page.getByText(`${followerHandle} にフォローされました`)).toBeVisible({ timeout: 15_000 });

  // 4. 投稿する。
  const postText = `Bsky配送テスト ${Date.now()}`;
  const createRes = await request.post("/api/notes/create", {
    headers: { Authorization: `Bearer ${user.token}` },
    data: { text: postText },
  });
  expect(createRes.ok(), `投稿作成失敗: ${createRes.status()} ${await createRes.text()}`).toBeTruthy();

  // 5. subscribeRepos に接続し、投稿の app.bsky.feed.post コミットが正しく流れることを確認する
  //    （配送の検証）。cursor=0 でDB永続化済みのイベントもリプレイされるため、接続タイミングの
  //    前後関係（投稿が先）を気にする必要はない。
  const wsUrl = `${SEIRAN_BASE_URL.replace("http://", "ws://")}/xrpc/com.atproto.sync.subscribeRepos?cursor=0`;
  const found = await waitForBskyFeedPost(wsUrl, did, postText, 15_000);
  expect(found, "subscribeRepos経由でapp.bsky.feed.postのコミットが確認できなかった").toBeTruthy();
});
