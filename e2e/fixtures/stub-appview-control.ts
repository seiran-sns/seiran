// stub-appview-server.ts の `/__control__/followers` を呼ぶだけの薄いヘルパー。
// テストごとに「対象DIDのフォロワー一覧」を丸ごと差し替えるために使う
// （Bsky側フォロワー検知ポーリング `bsky_follower_poll` のE2E検証、`getFollowers` レスポンスの差し替え）。

import { APIRequestContext } from "@playwright/test";

export interface FollowerEntry {
  did: string;
  handle: string;
  displayName?: string;
  avatar?: string;
}

export async function registerBskyFollowers(
  request: APIRequestContext,
  appviewBaseUrl: string,
  targetDid: string,
  followers: FollowerEntry[],
): Promise<void> {
  const res = await request.post(`${appviewBaseUrl}/__control__/followers`, {
    data: { targetDid, followers },
  });
  if (!res.ok()) {
    throw new Error(`registerBskyFollowers failed: ${res.status()} ${await res.text()}`);
  }
}
