import { useEffect, useRef, useState } from "react";

/**
 * 指定要素（`measureRef`）の内容が `containerRef` の幅に改行せず収まる最大のfont-sizeを
 * ResizeObserver＋二分探索で実測して返す（issue #243）。CSSのみでは任意長のHTML内容に対して
 * 正確なフィットができないため、実測ベースのアプローチを取る。
 */
export function useFitTextSize(maxPx: number, deps: readonly unknown[] = []) {
  const containerRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLSpanElement>(null);
  const [fontSize, setFontSize] = useState(maxPx);

  useEffect(() => {
    const container = containerRef.current;
    const measure = measureRef.current;
    if (!container || !measure) return;

    function fit() {
      if (!container || !measure) return;
      const available = container.clientWidth;
      if (available <= 0) return;

      let lo = 8;
      let hi = maxPx;
      measure.style.fontSize = `${hi}px`;
      if (measure.scrollWidth <= available) {
        setFontSize(hi);
        return;
      }
      while (hi - lo > 1) {
        const mid = Math.floor((lo + hi) / 2);
        measure.style.fontSize = `${mid}px`;
        if (measure.scrollWidth <= available) {
          lo = mid;
        } else {
          hi = mid;
        }
      }
      setFontSize(lo);
    }

    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(container);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [maxPx, ...deps]);

  return { containerRef, measureRef, fontSize };
}
