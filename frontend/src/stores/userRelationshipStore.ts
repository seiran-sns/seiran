import { useCallback, useSyncExternalStore } from "react";

export type FollowStatus = "not_following" | "pending" | "accepted";

/**
 * 閲覧者から見た「対ユーザー関係」（フォロー状態・ミュート・ブロック・リポストミュート）の
 * 外部ストア。プロフィール画面・NoteCardのフォロースイッチ・対ユーザー操作メニュー
 * （ProfilePageのケバブメニュー、NoteCardの右クリックメニュー）が共通で参照する。
 * 同一アクターへの関係表示は画面内に複数存在しうる（プロフィール本体＋タイムライン上の
 * 同一ユーザーの複数ポスト等）ため、ここに一本化し、更新は必ず `setRelationship` を
 * 経由させることで、表示中の全コンポーネントが同時に同期される（`followStatusStore.ts`の
 * 元々の設計思想をフォロー以外の4値にも拡張したもの）。
 *
 * キーは `lib/format.ts` の `profileQuery(username, domain)`（ローカルは domain 省略）で統一する。
 */
export interface RelationshipSnapshot {
  followStatus: FollowStatus;
  isMuted: boolean;
  isBlocking: boolean;
  isBlockedBy: boolean;
  isRepostMuted: boolean;
}

const relationshipMap = new Map<string, RelationshipSnapshot>();
const listeners = new Map<string, Set<() => void>>();

export function getRelationship(key: string): RelationshipSnapshot | undefined {
  return relationshipMap.get(key);
}

/** 権威ある上書き（プロフィール直接取得・アクション成功時・WebSocket受信）。
 * マージではなく全フィールド確定値で置換する。 */
export function setRelationship(key: string, snapshot: RelationshipSnapshot): void {
  relationshipMap.set(key, snapshot);
  listeners.get(key)?.forEach((cb) => cb());
}

/** ストアに未登録（＝一度もセットされていない）の場合のみ書き込む。タイムラインAPI
 * レスポンスに埋め込まれた relationship を NoteCard がマウント時に流し込む用途専用。
 * 既にユーザー操作やプロフィール取得で新しい値が入っている場合は上書きしない
 * （stale なタイムライン再描画で最新のミュート/ブロック操作結果が巻き戻る事故を防ぐ）。 */
export function seedRelationshipIfAbsent(key: string, snapshot: RelationshipSnapshot): void {
  if (relationshipMap.has(key)) return;
  setRelationship(key, snapshot);
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

/** 指定キーの現在の関係を購読する。ストアに未登録なら undefined（未取得を意味する）。 */
export function useRelationship(key: string): RelationshipSnapshot | undefined {
  const subscribeKey = useCallback((cb: () => void) => subscribe(key, cb), [key]);
  const getSnapshot = useCallback(() => getRelationship(key), [key]);
  return useSyncExternalStore(subscribeKey, getSnapshot);
}
