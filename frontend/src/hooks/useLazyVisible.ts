import { RefObject, useEffect, useState } from "react";

interface SharedEntry {
  observer: IntersectionObserver;
  callbacks: Map<Element, (visible: boolean) => void>;
}

// スクロールコンテナ（root）ごとに IntersectionObserver を 1 つだけ生成して使い回す。
// 絵文字ピッカーのように数千件のアイテムを描画する場面で要素ごとに observer を
// 生成すると生成コスト・メモリともに無視できなくなるため。
const rootedEntries = new WeakMap<Element, SharedEntry>();
let windowEntry: SharedEntry | null = null;

function createEntry(root: Element | null, rootMargin: string): SharedEntry {
  const callbacks = new Map<Element, (visible: boolean) => void>();
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        callbacks.get(entry.target)?.(entry.isIntersecting);
      }
    },
    { root, rootMargin }
  );
  return { observer, callbacks };
}

function getSharedEntry(root: Element | null, rootMargin: string): SharedEntry {
  if (!root) {
    if (!windowEntry) windowEntry = createEntry(null, rootMargin);
    return windowEntry;
  }
  let entry = rootedEntries.get(root);
  if (!entry) {
    entry = createEntry(root, rootMargin);
    rootedEntries.set(root, entry);
  }
  return entry;
}

/**
 * `elRef` が `rootRef`（スクロールコンテナ、`null`ならビューポート）内で可視かどうかを
 * 追跡する。同一 root 配下で多数の要素に使っても IntersectionObserver の生成は1回で済む。
 */
export function useLazyVisible(
  elRef: RefObject<Element | null>,
  rootRef: RefObject<Element | null>,
  rootMargin = "200px"
): boolean {
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    const { observer, callbacks } = getSharedEntry(rootRef.current, rootMargin);
    callbacks.set(el, setVisible);
    observer.observe(el);
    return () => {
      callbacks.delete(el);
      observer.unobserve(el);
    };
  }, [elRef, rootRef, rootMargin]);

  return visible;
}
