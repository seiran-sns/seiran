import { useCallback, useSyncExternalStore } from "react";
import type { ReactionSummary } from "../api/client";

/**
 * ポストの絵文字リアクション集計をノートIDごとに保持する外部ストア。
 * 元々は NoteCard（useNoteCardActions）内のローカル state だったため、WebSocket経由の
 * リアルタイム更新（他人のリアクション追加/切替/取消）がそのコンポーネントインスタンスにしか
 * 反映されず、HomeFeedContext のタイムラインキャッシュ（Note配列そのものをキャッシュする）
 * には伝播しなかった。その結果、タイムライン表示中にリアクションが更新されても、他画面へ
 * 遷移してブラウザバックで戻ると「リアクション未反映時点のキャッシュ」から復元され、リアクションが
 * 消えて見える不具合があった（リロードすればサーバーの最新値を取り直すため直る）。
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

/** 指定ノートの現在のリアクション集計を購読する。ストア未登録なら `initialReactions` でシードする。 */
export function useReactionState(
  noteId: string,
  initialReactions: ReactionSummary[] | undefined
): ReactionSummary[] {
  if (!states.has(noteId)) {
    states.set(noteId, initialReactions ?? []);
  }
  const subscribeNote = useCallback(
    (listener: () => void) => subscribeReactionState(noteId, listener),
    [noteId]
  );
  const getSnapshot = useCallback(() => states.get(noteId)!, [noteId]);
  return useSyncExternalStore(subscribeNote, getSnapshot, getSnapshot);
}
