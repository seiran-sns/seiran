import { useSyncExternalStore } from "react";

/**
 * リポスト・リアクションの連打を止めるクールダウン（バックエンドの
 * `interaction_cooldown` と同じ10秒）。同じポストを表示しているカードが複数あっても
 * 揃って止まるよう、カードごとの state ではなくモジュール単位で持つ。
 * バックエンドも同じ判定で 429 を返すので、ここは無駄な送信とエラー表示を減らすためのもの。
 */
export const INTERACTION_COOLDOWN_MS = 10_000;

const until = new Map<string, number>();
const listeners = new Set<() => void>();

export function repostCooldownKey(noteId: string): string {
  return `repost:${noteId}`;
}

/** 絵文字を続けて付けて文にする使い方があるので、リアクションは絵文字ごとに分ける。 */
export function reactionCooldownKey(noteId: string, emoji: string): string {
  return `reaction:${noteId}:${emoji}`;
}

function notify() {
  for (const l of listeners) l();
}

export function startCooldown(key: string, ms: number = INTERACTION_COOLDOWN_MS) {
  const expiresAt = Date.now() + ms;
  until.set(key, expiresAt);
  notify();
  setTimeout(() => {
    if (until.get(key) === expiresAt) {
      until.delete(key);
      notify();
    }
  }, ms);
}

export function isCoolingDown(key: string): boolean {
  const expiresAt = until.get(key);
  return expiresAt !== undefined && expiresAt > Date.now();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** `key` がクールダウン中か（終わると再描画される）。 */
export function useCoolingDown(key: string): boolean {
  return useSyncExternalStore(subscribe, () => isCoolingDown(key));
}
