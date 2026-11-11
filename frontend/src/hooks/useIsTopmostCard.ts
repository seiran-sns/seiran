import { RefObject, useEffect, useState } from "react";

/**
 * 上端が画面内に見えているカード（`<article>`）のうち、文書順で最初の1枚を「いちばん上」とする。
 * タッチ環境のクイックリアクションのポップアップを、そのカードにだけ出すのに使う（マウス環境は
 * ホバーで出すので使わない）。
 *
 * - 画面に少しでも入っているカードを `IntersectionObserver` で絞り、その数枚だけをスクロールの
 *   たびに（`requestAnimationFrame` で間引いて）調べる。タイムラインが長くなっても調べる数は増えない。
 * - 上端が画面の上へ出ていれば見えていない。張り付いたタブ・ヘッダー（`position: sticky`）の下に
 *   隠れている場合も見えていないので、上端のすぐ下の点に実際にそのカードが描かれているかを
 *   `elementFromPoint` で確かめる。点は上端から `PROBE_OFFSET_PX` 下にする（上のカードの
 *   ポップアップが次のカードの上端にわずかに重なるので、上端ちょうどでは重なりを拾う）。
 */

const PROBE_OFFSET_PX = 8;

const registered = new Map<Element, (topmost: boolean) => void>();
const onScreen = new Set<Element>();
let topmost: Element | null = null;
let observer: IntersectionObserver | null = null;
let frame: number | null = null;

function topEdgeVisible(card: Element): boolean {
  const rect = card.getBoundingClientRect();
  const y = rect.top + PROBE_OFFSET_PX;
  if (rect.top < 0 || y >= window.innerHeight) return false;
  const hit = document.elementFromPoint(rect.left + rect.width / 2, y);
  return hit !== null && card.contains(hit);
}

function recompute() {
  frame = null;
  const candidates = [...onScreen].sort((a, b) =>
    a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1
  );
  const next = candidates.find(topEdgeVisible) ?? null;
  if (next === topmost) return;
  const prev = topmost;
  topmost = next;
  if (prev) registered.get(prev)?.(false);
  if (next) registered.get(next)?.(true);
}

function scheduleRecompute() {
  if (frame === null) frame = window.requestAnimationFrame(recompute);
}

function start() {
  if (observer || typeof IntersectionObserver === "undefined") return;
  observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (entry.isIntersecting) onScreen.add(entry.target);
      else onScreen.delete(entry.target);
    }
    scheduleRecompute();
  });
  window.addEventListener("scroll", scheduleRecompute, { passive: true });
  window.addEventListener("resize", scheduleRecompute, { passive: true });
}

function stopIfIdle() {
  if (registered.size > 0 || !observer) return;
  observer.disconnect();
  observer = null;
  window.removeEventListener("scroll", scheduleRecompute);
  window.removeEventListener("resize", scheduleRecompute);
  if (frame !== null) window.cancelAnimationFrame(frame);
  frame = null;
}

/** `ref`（カードの中の要素）を含むカードが、`enabled` の間、上端が見えているカードのうち文書順の最初か。 */
export function useIsTopmostCard(ref: RefObject<Element>, enabled: boolean): boolean {
  const [isTopmost, setIsTopmost] = useState(false);

  useEffect(() => {
    const card = ref.current?.closest("article") ?? null;
    if (!enabled || !card) {
      setIsTopmost(false);
      return;
    }
    start();
    if (!observer) return;
    registered.set(card, setIsTopmost);
    observer.observe(card);
    return () => {
      observer?.unobserve(card);
      registered.delete(card);
      onScreen.delete(card);
      if (topmost === card) {
        topmost = null;
        scheduleRecompute();
      }
      stopIfIdle();
    };
  }, [ref, enabled]);

  return isTopmost;
}
