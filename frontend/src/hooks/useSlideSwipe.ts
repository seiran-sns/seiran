import { RefObject, TouchEvent as ReactTouchEvent, useCallback, useEffect, useRef, useState } from "react";

/** 1 = 次（指を左へ動かし、右から入ってくる）、-1 = 前。 */
export type SlideSide = 1 | -1;

/**
 * - `idle`: 何もしていない（表示中の一覧に transform を付けない）
 * - `dragging`: 指に追従中
 * - `animating`: 指を離した後の切り替え・戻りのアニメーション中
 * - `committed`: 切り替えのアニメーションが終わり、親が新しい一覧を出すのを待っている
 */
export type SlidePhase = "idle" | "dragging" | "animating" | "committed";

interface SlideSwipeOptions {
  /** 横にスライドさせる範囲。ここで非 passive の touchmove を受け、横スワイプ中の縦スクロールを止める。 */
  sliderRef: RefObject<HTMLElement | null>;
  /** 表示中の一覧。 */
  trackRef: RefObject<HTMLElement | null>;
  /** 隣の一覧（親が `onNeighborChange` を受けて描画する）。 */
  paneRef: RefObject<HTMLElement | null>;
  hasNeighbor: (side: SlideSide) => boolean;
  /** 隣として描くものが変わった。`null` は隣を描かなくてよい。 */
  onNeighborChange: (side: SlideSide | null) => void;
  /** 隣が中央まで来た。親は切り替えて、新しい一覧を出せたら `finish()` を呼ぶ。 */
  onCommit: (side: SlideSide) => void;
  /** 切り替えとみなす最小の横移動（px）。 */
  minDistance?: number;
  /** 縦移動に対する横移動の最小比率。これ未満で動き出したら縦スクロールとして扱う。 */
  maxVerticalRatio?: number;
}

/** 縦か横かを決めるまでの遊び（px）。 */
const LOCK_SLOP_PX = 10;
/** 隣が無い向きへ引いたときの追従の割合。 */
const RUBBER_BAND = 0.3;
const DURATION_MS = 220;

interface Gesture {
  startX: number;
  startY: number;
  mode: "pending" | "horizontal";
  side: SlideSide | null;
  hasTarget: boolean;
  offset: number;
}

function prefersReducedMotion(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * 横スワイプで隣の一覧へ切り替える。表示中の一覧と隣の一覧を指に合わせて横へ動かし、
 * 離した位置で切り替えるか戻すかを決める。
 *
 * 動かすのは DOM の style へ直接書く（指の動きごとに React で描き直すと、一覧全体の再描画で
 * 追従が遅れる）。止まっている間は transform を外す（transform のある祖先の中では
 * position: fixed の子孫（ライトボックス等）が画面ではなくその祖先基準になる）。
 *
 * 開始は React の onTouchStart で受ける: ライトボックスはノートの中に描かれ、React の
 * タッチイベントの伝播を止めてスワイプを始めさせない。
 */
export function useSlideSwipe({
  sliderRef,
  trackRef,
  paneRef,
  hasNeighbor,
  onNeighborChange,
  onCommit,
  minDistance = 50,
  maxVerticalRatio = 1.5,
}: SlideSwipeOptions) {
  const [phase, setPhaseState] = useState<SlidePhase>("idle");
  const phaseRef = useRef<SlidePhase>("idle");
  const setPhase = useCallback((p: SlidePhase) => {
    phaseRef.current = p;
    setPhaseState(p);
  }, []);
  const gesture = useRef<Gesture | null>(null);
  // 今の位置（隣の一覧が後から描かれたとき、同じ位置へ置くため）。
  const position = useRef<{ track: string; pane: string; transition: string } | null>(null);
  const callbacks = useRef({ hasNeighbor, onNeighborChange, onCommit });
  callbacks.current = { hasNeighbor, onNeighborChange, onCommit };

  const apply = useCallback(
    (track: string, pane: string, animate: boolean) => {
      const transition = animate ? `transform ${DURATION_MS}ms ease-out` : "none";
      position.current = { track, pane, transition };
      for (const [el, transform] of [
        [trackRef.current, track],
        [paneRef.current, pane],
      ] as const) {
        if (!el) continue;
        el.style.transition = transition;
        el.style.transform = transform;
      }
    },
    [trackRef, paneRef],
  );

  /** 隣の一覧が描かれた・描き直されたときに、今の位置へ置き直す。 */
  const syncPane = useCallback(() => {
    const el = paneRef.current;
    const p = position.current;
    if (!el || !p) return;
    el.style.transition = "none";
    el.style.transform = p.pane;
  }, [paneRef]);

  const clear = useCallback(() => {
    position.current = null;
    for (const el of [trackRef.current, paneRef.current]) {
      if (!el) continue;
      el.style.transition = "";
      el.style.transform = "";
    }
  }, [trackRef, paneRef]);

  // アニメーションの終わり。transitionend が来ない場合（タブが裏に回った・動きを減らす設定）にも
  // 進むようにタイマーを併用し、どちらか先に来たほうで1回だけ進める。
  const afterAnimation = useCallback(
    (done: () => void) => {
      const track = trackRef.current;
      let finished = false;
      const finishOnce = () => {
        if (finished) return;
        finished = true;
        track?.removeEventListener("transitionend", onEnd);
        window.clearTimeout(timer);
        done();
      };
      const onEnd = (e: TransitionEvent) => {
        if (e.target === track && e.propertyName === "transform") finishOnce();
      };
      track?.addEventListener("transitionend", onEnd);
      const timer = window.setTimeout(finishOnce, prefersReducedMotion() ? 0 : DURATION_MS + 100);
    },
    [trackRef],
  );

  const springBack = useCallback(
    (side: SlideSide | null) => {
      setPhase("animating");
      apply("translateX(0px)", `translateX(${(side ?? 1) * 100}%)`, true);
      afterAnimation(() => {
        clear();
        setPhase("idle");
        callbacks.current.onNeighborChange(null);
      });
    },
    [apply, afterAnimation, clear, setPhase],
  );

  const commit = useCallback(
    (side: SlideSide) => {
      setPhase("animating");
      apply(`translateX(${-side * 100}%)`, "translateX(0px)", true);
      afterAnimation(() => {
        setPhase("committed");
        callbacks.current.onCommit(side);
      });
    },
    [apply, afterAnimation, setPhase],
  );

  /** 親が新しい一覧を出せた（または切り替えをやめた）。 */
  const finish = useCallback(() => {
    clear();
    setPhase("idle");
    callbacks.current.onNeighborChange(null);
  }, [clear, setPhase]);

  const onTouchStart = useCallback((e: ReactTouchEvent) => {
    if (phaseRef.current !== "idle" || e.touches.length !== 1) {
      gesture.current = null;
      return;
    }
    const t = e.touches[0];
    gesture.current = { startX: t.clientX, startY: t.clientY, mode: "pending", side: null, hasTarget: false, offset: 0 };
  }, []);

  useEffect(() => {
    const slider = sliderRef.current;
    if (!slider) return;

    const onTouchMove = (e: TouchEvent) => {
      const g = gesture.current;
      if (!g) return;
      if (e.touches.length !== 1) {
        gesture.current = null;
        if (g.mode === "horizontal") springBack(g.side);
        return;
      }
      const dx = e.touches[0].clientX - g.startX;
      const dy = e.touches[0].clientY - g.startY;
      if (g.mode === "pending") {
        if (Math.max(Math.abs(dx), Math.abs(dy)) < LOCK_SLOP_PX) return;
        if (Math.abs(dx) <= Math.abs(dy) * maxVerticalRatio) {
          gesture.current = null;
          return;
        }
        g.mode = "horizontal";
        setPhase("dragging");
      }
      if (e.cancelable) e.preventDefault();
      if (dx === 0) return;
      const side: SlideSide = dx < 0 ? 1 : -1;
      if (side !== g.side) {
        g.side = side;
        g.hasTarget = callbacks.current.hasNeighbor(side);
        callbacks.current.onNeighborChange(g.hasTarget ? side : null);
      }
      g.offset = g.hasTarget ? dx : dx * RUBBER_BAND;
      apply(`translateX(${g.offset}px)`, `translateX(calc(${side * 100}% + ${g.offset}px))`, false);
    };

    const release = (canceled: boolean) => {
      const g = gesture.current;
      gesture.current = null;
      if (!g || g.mode !== "horizontal") return;
      if (!canceled && g.side !== null && g.hasTarget && Math.abs(g.offset) > minDistance) commit(g.side);
      else springBack(g.side);
    };
    const onTouchEnd = () => release(false);
    const onTouchCancel = () => release(true);

    slider.addEventListener("touchmove", onTouchMove, { passive: false });
    slider.addEventListener("touchend", onTouchEnd, { passive: true });
    slider.addEventListener("touchcancel", onTouchCancel, { passive: true });
    return () => {
      slider.removeEventListener("touchmove", onTouchMove);
      slider.removeEventListener("touchend", onTouchEnd);
      slider.removeEventListener("touchcancel", onTouchCancel);
    };
  }, [sliderRef, apply, commit, springBack, setPhase, minDistance, maxVerticalRatio]);

  return { phase, onTouchStart, syncPane, finish };
}
