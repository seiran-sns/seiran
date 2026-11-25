import { useSyncExternalStore } from "react";

/**
 * 設定画面「表示」＞「この端末だけの設定」のオン/オフ1つ分。見やすさや通信量の好みは端末（画面・回線）
 * 次第なので、アカウントでなくこの端末の `localStorage` に `"1"` / 無し で持つ。別のタブで切り替えた
 * ときも `storage` イベントで開いているタブに反映する。
 */
export interface DeviceFlag {
  get(): boolean;
  set(next: boolean): void;
  use(): boolean;
}

export function createDeviceFlag(storageKey: string, onChange?: (enabled: boolean) => void): DeviceFlag {
  function readStored(): boolean {
    try {
      return localStorage.getItem(storageKey) === "1";
    } catch {
      return false;
    }
  }

  let enabled = readStored();
  const listeners = new Set<() => void>();

  function notify() {
    onChange?.(enabled);
    listeners.forEach((l) => l());
  }

  function subscribe(listener: () => void) {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }

  if (typeof window !== "undefined") {
    window.addEventListener("storage", (e) => {
      if (e.key !== storageKey) return;
      enabled = readStored();
      notify();
    });
  }

  return {
    get: () => enabled,
    set(next: boolean) {
      enabled = next;
      try {
        if (next) localStorage.setItem(storageKey, "1");
        else localStorage.removeItem(storageKey);
      } catch {
        // 保存できない環境（プライベートブラウズ等）でも、このタブの間は効かせる。
      }
      notify();
    },
    use: () => useSyncExternalStore(subscribe, () => enabled),
  };
}
