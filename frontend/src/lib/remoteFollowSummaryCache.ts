import { api, RemoteFollowSummaryResponse } from "../api/client";

/**
 * リモートFediアクターのフォロー中/フォロワー全件取得（#68）を、プロフィール画面ロードの
 * できるだけ初期段階で先読みしておくためのモジュールスコープキャッシュ。
 *
 * マイケル指摘: 従来は `FollowListPanel`（フォロー中/フォロワータブの中身）がマウントされた
 * 時点、つまりユーザーがタブを開いた瞬間に初めてAPIコールが飛んでいたため、タブを開くたびに
 * 「読み込み中」が待たされた感を出していた。`ProfilePage` がプロフィール取得完了直後（タブが
 * まだ投稿一覧でも）に `prefetchRemoteFollowSummary` を呼んでおくことで、実際にタブが開かれる
 * 頃には取得が完了しているか、少なくとも進行中になっている。
 */
const cache = new Map<string, Promise<RemoteFollowSummaryResponse>>();

function cacheKey(actorId: string, direction: "following" | "followers"): string {
  return `${actorId}:${direction}`;
}

function fetchAndCache(actorId: string, direction: "following" | "followers"): Promise<RemoteFollowSummaryResponse> {
  const key = cacheKey(actorId, direction);
  const promise = api.users.remoteFollowSummary(actorId, direction).catch((e) => {
    // 失敗したPromiseをキャッシュに残すと、再訪時も永久に失敗し続けるため取り除く。
    cache.delete(key);
    throw e;
  });
  cache.set(key, promise);
  return promise;
}

/** プロフィール画面ロード直後の先読み開始。既にキャッシュがあれば何もしない。 */
export function prefetchRemoteFollowSummary(actorId: string, direction: "following" | "followers"): void {
  if (!cache.has(cacheKey(actorId, direction))) {
    fetchAndCache(actorId, direction);
  }
}

/** キャッシュ済みなら再利用し、無ければ新規に取得する（`FollowListPanel` から呼ぶ）。 */
export function getRemoteFollowSummary(actorId: string, direction: "following" | "followers"): Promise<RemoteFollowSummaryResponse> {
  return cache.get(cacheKey(actorId, direction)) ?? fetchAndCache(actorId, direction);
}
