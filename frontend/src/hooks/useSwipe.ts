import { TouchEvent, useRef } from "react";

interface SwipeOptions {
  onSwipeLeft?: () => void;
  onSwipeRight?: () => void;
  /** スワイプ判定とする最小ピクセル距離 (デフォルト: 50) */
  minDistance?: number;
  /** 縦移動に対する横移動の最小比率。縦スクロールとの誤判定を防ぐ (デフォルト: 1.5) */
  maxVerticalRatio?: number;
}

export function useSwipe({
  onSwipeLeft,
  onSwipeRight,
  minDistance = 50,
  maxVerticalRatio = 1.5,
}: SwipeOptions) {
  const startX = useRef<number | null>(null);
  const startY = useRef<number | null>(null);

  const onTouchStart = (e: TouchEvent) => {
    if (e.touches.length !== 1) return;
    startX.current = e.touches[0].clientX;
    startY.current = e.touches[0].clientY;
  };

  const onTouchEnd = (e: TouchEvent) => {
    if (startX.current === null || startY.current === null) return;
    if (e.changedTouches.length !== 1) return;

    const endX = e.changedTouches[0].clientX;
    const endY = e.changedTouches[0].clientY;

    const deltaX = endX - startX.current;
    const deltaY = endY - startY.current;

    startX.current = null;
    startY.current = null;

    const absX = Math.abs(deltaX);
    const absY = Math.abs(deltaY);

    if (absX >= minDistance && absX > absY * maxVerticalRatio) {
      if (deltaX < 0) {
        onSwipeLeft?.();
      } else {
        onSwipeRight?.();
      }
    }
  };

  return { onTouchStart, onTouchEnd };
}
