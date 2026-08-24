import { useSyncExternalStore } from "react";

/**
 * 1秒ごとに時刻を更新する共有タイマーストア（#228 アンケート残り時間表示）。画面上の
 * 複数コンポーネントが同時に「1秒ごとに再計算したい」場合でも、setIntervalは購読者が
 * 1人以上いる間だけ1本に集約して動かす（イベント駆動での再描画。購読者が0人になれば
 * 停止し、無駄なタイマーを残さない）。
 */
let currentTime = Date.now();
const listeners = new Set<() => void>();
let intervalId: ReturnType<typeof setInterval> | null = null;

function tick(): void {
  currentTime = Date.now();
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (intervalId === null) {
    intervalId = setInterval(tick, 1000);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && intervalId !== null) {
      clearInterval(intervalId);
      intervalId = null;
    }
  };
}

function getSnapshot(): number {
  return currentTime;
}

/** 1秒ごとに更新される現在時刻（epoch ms）を返す。呼び出し中のコンポーネントは
 * 購読している間、1秒ごとに再レンダリングされる。 */
export function useSecondTicker(): number {
  return useSyncExternalStore(subscribe, getSnapshot);
}
