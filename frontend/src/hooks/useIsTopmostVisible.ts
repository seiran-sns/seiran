import { RefObject, useEffect, useState } from "react";

/**
 * 登録した要素のうち、画面内に全体が見えているものの中で文書順で最初の1つを「いちばん上」とする。
 * タッチ環境のクイックリアクションのポップアップを、画面内でいちばん上の NoteCard にだけ出すのに使う
 * （マウス環境はホバーで出すので使わない）。タイムラインの並びは文書順なので、位置を測らずに
 * 交差状態の変化（`IntersectionObserver`）だけで決まり、スクロールのたびに計算しない。
 */

const fullyVisible = new Set<Element>();
const listeners = new Map<Element, (topmost: boolean) => void>();
let topmost: Element | null = null;
let observer: IntersectionObserver | null = null;

function recompute() {
  let first: Element | null = null;
  for (const el of fullyVisible) {
    if (!first || first.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_PRECEDING) first = el;
  }
  if (first === topmost) return;
  const prev = topmost;
  topmost = first;
  if (prev) listeners.get(prev)?.(false);
  if (first) listeners.get(first)?.(true);
}

function getObserver(): IntersectionObserver | null {
  if (observer || typeof IntersectionObserver === "undefined") return observer;
  observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.intersectionRatio >= 0.99) fullyVisible.add(entry.target);
        else fullyVisible.delete(entry.target);
      }
      recompute();
    },
    { threshold: [0, 0.99] }
  );
  return observer;
}

/** `ref` の要素が、`enabled` の間、画面内に全体が見えている登録要素の中で文書順の最初か。 */
export function useIsTopmostVisible(ref: RefObject<Element>, enabled: boolean): boolean {
  const [isTopmost, setIsTopmost] = useState(false);

  useEffect(() => {
    const el = ref.current;
    const obs = enabled ? getObserver() : null;
    if (!el || !obs) {
      setIsTopmost(false);
      return;
    }
    listeners.set(el, setIsTopmost);
    obs.observe(el);
    return () => {
      obs.unobserve(el);
      listeners.delete(el);
      fullyVisible.delete(el);
      if (topmost === el) {
        topmost = null;
        recompute();
      }
    };
  }, [ref, enabled]);

  return isTopmost;
}
