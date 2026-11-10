import { RefObject, useLayoutEffect, useRef, useState } from "react";

/** まだ一度も表示していない（幅を測れていない）要素の幅の見積もり（px）。 */
const UNMEASURED_WIDTH = 32;

/**
 * 横並びの行（flex、`gap` あり）のうち、`data-fit-key` を付けた子要素を先頭から何個まで
 * 表示できるかを返す。`data-fit-key` の無い子要素は常に表示する前提で先に幅を確保し、
 * 残りの幅に収まる個数だけを返す（呼び出し側は残りを `display: none` にする）。
 * 収まらない分を折り返し・クリップで見せないのではなく数を返すのは、隠した要素が
 * 行の幅を占めたりキーボードフォーカスを受けたりしないようにするため。
 *
 * 一度表示した要素の幅を覚えておき、隠している要素はその幅で収まるかを判定する。
 */
export function useFitCount(rowRef: RefObject<HTMLElement>, keys: string[]): number {
  const [fit, setFit] = useState(keys.length);
  const widths = useRef(new Map<string, number>());
  const keysSig = keys.join("\n");

  useLayoutEffect(() => {
    const row = rowRef.current;
    if (!row) return;
    const keyList = keysSig === "" ? [] : keysSig.split("\n");

    function compute() {
      if (!row) return;
      const gap = parseFloat(getComputedStyle(row).columnGap) || 0;
      let used = 0;
      let fixedCount = 0;
      for (const child of Array.from(row.children) as HTMLElement[]) {
        const width = child.getBoundingClientRect().width;
        const key = child.dataset.fitKey;
        if (key !== undefined) {
          if (width > 0) widths.current.set(key, width);
          continue;
        }
        used += width;
        fixedCount += 1;
      }
      used += gap * Math.max(fixedCount - 1, 0);
      const available = row.getBoundingClientRect().width + 0.5;
      const known = [...widths.current.values()];
      const fallback = known.length > 0 ? Math.max(...known) : UNMEASURED_WIDTH;
      let count = 0;
      for (const key of keyList) {
        const next = used + gap + (widths.current.get(key) ?? fallback);
        if (next > available) break;
        used = next;
        count += 1;
      }
      setFit(count);
    }

    compute();
    if (typeof ResizeObserver === "undefined") return;
    // 行自体の幅の変化に加え、常に表示する子要素（件数表示の増減で幅が変わる）も監視する。
    const observer = new ResizeObserver(compute);
    observer.observe(row);
    for (const child of Array.from(row.children) as HTMLElement[]) {
      if (child.dataset.fitKey === undefined) observer.observe(child);
    }
    return () => observer.disconnect();
  }, [rowRef, keysSig]);

  return Math.min(fit, keys.length);
}
