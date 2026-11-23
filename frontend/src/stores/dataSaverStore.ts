import { useSyncExternalStore } from "react";

/**
 * ギガ節約モード（設定画面「表示」＞「この端末だけの設定」）。オンの間、投稿に添付された画像・動画を
 * クリックするまで読み込まない。通信量を気にするかは回線（端末）次第なので、アカウントでなく
 * この端末の `localStorage` に持つ。
 */
export const DATA_SAVER_STORAGE_KEY = "seiran_data_saver";

function readStored(): boolean {
  try {
    return localStorage.getItem(DATA_SAVER_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

let enabled = readStored();
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// 別タブで切り替えたときも、開いているタブのタイムラインに反映する。
if (typeof window !== "undefined") {
  window.addEventListener("storage", (e) => {
    if (e.key !== DATA_SAVER_STORAGE_KEY) return;
    enabled = readStored();
    listeners.forEach((l) => l());
  });
}

export function setDataSaver(next: boolean) {
  enabled = next;
  try {
    if (next) localStorage.setItem(DATA_SAVER_STORAGE_KEY, "1");
    else localStorage.removeItem(DATA_SAVER_STORAGE_KEY);
  } catch {
    // 保存できない環境（プライベートブラウズ等）でも、このタブの間は効かせる。
  }
  listeners.forEach((l) => l());
}

export function useDataSaver(): boolean {
  return useSyncExternalStore(subscribe, () => enabled);
}
