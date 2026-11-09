import { useCallback, useSyncExternalStore } from "react";
import type { ReactionSummary } from "../api/client";

/**
 * ポストの絵文字リアクション集計をノートIDごとに保持する外部ストア。
 * コンポーネントのローカル state にすると、WebSocket経由の更新がそのインスタンスにしか反映されず、
 * HomeFeedContext のタイムラインキャッシュから復元したときにリアクションが消えて見える。
 * pollVoteStore/userRelationshipStore と同じ「IDキーの外部ストア」パターンに寄せることで、
 * 同じノートを表示するどのコンポーネントインスタンスからも常に最新値が見えるようにする。
 */
const states = new Map<string, ReactionSummary[]>();
const listeners = new Map<string, Set<() => void>>();

export function getReactionState(noteId: string): ReactionSummary[] | undefined {
  return states.get(noteId);
}

export function setReactionState(noteId: string, reactions: ReactionSummary[]): void {
  states.set(noteId, reactions);
  listeners.get(noteId)?.forEach((listener) => listener());
}

export function subscribeReactionState(noteId: string, listener: () => void): () => void {
  let noteListeners = listeners.get(noteId);
  if (!noteListeners) {
    noteListeners = new Set();
    listeners.set(noteId, noteListeners);
  }
  noteListeners.add(listener);
  return () => {
    noteListeners!.delete(listener);
    if (noteListeners!.size === 0) listeners.delete(noteId);
  };
}

const EMPTY: ReactionSummary[] = [];

/** 指定ノートの現在のリアクション集計を購読する。ストア未登録なら `initialReactions` でシードする。
 * `noteId` が null なら何も購読せず常に空配列を返す（フックを条件付きで呼べないため、
 * 「対象が無いかもしれない」購読はこの形で表す）。 */
export function useReactionState(
  noteId: string | null,
  initialReactions: ReactionSummary[] | undefined
): ReactionSummary[] {
  if (noteId !== null && !states.has(noteId)) {
    states.set(noteId, initialReactions ?? []);
  }
  const subscribeNote = useCallback(
    (listener: () => void) =>
      noteId === null ? () => {} : subscribeReactionState(noteId, listener),
    [noteId]
  );
  const getSnapshot = useCallback(
    () => (noteId === null ? EMPTY : states.get(noteId)!),
    [noteId]
  );
  return useSyncExternalStore(subscribeNote, getSnapshot, getSnapshot);
}
