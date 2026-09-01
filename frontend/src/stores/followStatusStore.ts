import {
  getRelationship,
  RelationshipSnapshot,
  setRelationship,
  useRelationship,
} from "./userRelationshipStore";

export type { FollowStatus } from "./userRelationshipStore";
import type { FollowStatus } from "./userRelationshipStore";

/**
 * フォロー状態専用の薄いファサード（`stores/userRelationshipStore.ts`への委譲）。
 * 元々はフォロー状態単体の外部ストアだったが、ミュート・ブロック・リポストミュートを
 * 含む「対ユーザー関係」全体を1つのストアに統合したため、既存の狭いAPI
 * （`getFollowStatus`/`setFollowStatus`/`useFollowStatus`）はここに残し、
 * 呼び出し元（`StreamingContext.tsx`・`ProfilePage.tsx`・`NoteCard.tsx`）を無改修にする。
 */
const DEFAULT_OTHERS: Omit<RelationshipSnapshot, "followStatus"> = {
  isMuted: false,
  isBlocking: false,
  isBlockedBy: false,
  isRepostMuted: false,
};

export function getFollowStatus(key: string): FollowStatus | undefined {
  return getRelationship(key)?.followStatus;
}

/** フォロー操作の成功時・WebSocket `followAccepted` 受信時に呼び、購読中の全コンポーネントへ伝播させる。 */
export function setFollowStatus(key: string, status: FollowStatus): void {
  const prev = getRelationship(key);
  setRelationship(key, prev ? { ...prev, followStatus: status } : { followStatus: status, ...DEFAULT_OTHERS });
}

/** 指定キーの現在のフォロー状態を購読する。ストアに未登録なら undefined（未取得を意味する）。 */
export function useFollowStatus(key: string): FollowStatus | undefined {
  return useRelationship(key)?.followStatus;
}
