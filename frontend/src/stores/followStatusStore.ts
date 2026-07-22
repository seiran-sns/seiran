import { useCallback, useSyncExternalStore } from "react";

export type FollowStatus = "not_following" | "pending" | "accepted";

/**
 * フォロー状態の外部ストア（プロフィール画面・NoteCardのフォロースイッチが共通で参照する）。
 * 同一アクターへのフォロー状態表示は画面内に複数存在しうる（プロフィール本体＋右ペインの
 * ポストリスト、タイムライン上の同一ユーザーの複数ポスト等）。各コンポーネントが自分の
 * ローカル state だけで持つと、一方を操作しても他方に伝播せず「もう一度開くと古い状態に
 * 戻る」ような食い違いが起きる。ここに一本化し、更新は必ず `setFollowStatus` を経由させることで、
 * 表示中の全コンポーネントが同時に同期される。
 *
 * キーは `lib/format.ts` の `profileQuery(username, domain)`（ローカルは domain 省略）で統一する。
 */
const statusMap = new Map<string, FollowStatus>();
const listeners = new Map<string, Set<() => void>>();

export function getFollowStatus(key: string): FollowStatus | undefined {
  return statusMap.get(key);
}

/** フォロー操作の成功時・WebSocket `followAccepted` 受信時に呼び、購読中の全コンポーネントへ伝播させる。 */
export function setFollowStatus(key: string, status: FollowStatus): void {
  statusMap.set(key, status);
  listeners.get(key)?.forEach((cb) => cb());
}

function subscribe(key: string, cb: () => void): () => void {
  let set = listeners.get(key);
  if (!set) {
    set = new Set();
    listeners.set(key, set);
  }
  set.add(cb);
  return () => {
    set!.delete(cb);
    if (set!.size === 0) listeners.delete(key);
  };
}

/** 指定キーの現在のフォロー状態を購読する。ストアに未登録なら undefined（未取得を意味する）。 */
export function useFollowStatus(key: string): FollowStatus | undefined {
  const subscribeKey = useCallback((cb: () => void) => subscribe(key, cb), [key]);
  const getSnapshot = useCallback(() => getFollowStatus(key), [key]);
  return useSyncExternalStore(subscribeKey, getSnapshot);
}
