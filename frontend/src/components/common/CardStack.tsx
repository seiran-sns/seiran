import { HTMLAttributes, ReactNode, Ref, useEffect, useRef, useState } from "react";
import styles from "./CardStack.module.css";

/** 重ねたカードを縦にずらす量（px）。 */
const STACK_OFFSET_PX = 8;
/** タッチ環境で前面に出すカードを決める線の位置（画面の高さに対する割合、上から）。 */
const TOUCH_FOCUS_LINE = 0.25;

function useNoHover(): boolean {
  const [noHover, setNoHover] = useState(() => window.matchMedia?.("(hover: none)").matches ?? false);
  useEffect(() => {
    const mql = window.matchMedia?.("(hover: none)");
    if (!mql) return;
    const onChange = () => setNoHover(mql.matches);
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);
  return noHover;
}

/**
 * まとめたカード（通知欄の `lib/notificationGroups.ts`、タイムラインの `lib/timelineGroups.ts`）を、
 * 縦に `STACK_OFFSET_PX` ずつずらして重ねる。`keys` は上（新しい）から順。
 *
 * 前面に出すカード: マウス環境は最後にマウスが乗ったカード（離れても前面のまま残す）、タッチ環境は画面の上から
 * `TOUCH_FOCUS_LINE` の線にかかっているカード（まとまりの高さを件数で等分し、線の位置に
 * 当たるカード）で、まとまり全体が線より上にあれば一番古いカード。どちらでもなければ
 * 一番新しいカード。
 *
 * 上下関係は前面のカードに近いほど上にする（1〜5 で 3 が前面なら 3 > 2 = 4 > 1 = 5）。前面より
 * 上のカードは上端の帯、下のカードは下端の帯が見えるので、全部のカードの帯が見える。
 * z-index は使わず、遠いカードほど DOM の前に置いて描画順で重ねる。z-index を付けるとカードごとに
 * 重なりの文脈ができ、中のホバープレビュー（position: fixed）が左ペイン等の下に潜る。同じ理由で
 * ずらしも transform ではなく top で行う（transform の中では position: fixed の基準がカードになる）。
 *
 * カードは一番高いカードの高さまで伸ばす（`.stackCard` の直下の要素）。
 */
export default function CardStack({
  keys,
  renderCard,
  as: Tag = "div",
  className,
  containerProps,
}: {
  keys: string[];
  renderCard: (index: number) => ReactNode;
  as?: "div" | "li";
  className?: string;
  containerProps?: HTMLAttributes<HTMLElement> & Record<`data-${string}`, string>;
}) {
  const ref = useRef<HTMLElement>(null);
  const noHover = useNoHover();
  const [hovered, setHovered] = useState<number | null>(null);
  const [touchFocused, setTouchFocused] = useState<number | null>(null);
  const count = keys.length;

  useEffect(() => {
    if (!noHover) return;
    let frame: number | null = null;
    const update = () => {
      frame = null;
      const el = ref.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      const line = window.innerHeight * TOUCH_FOCUS_LINE;
      if (line < rect.top || rect.height <= 0) {
        setTouchFocused(null);
        return;
      }
      // 線より上へ通り過ぎたまとまりは一番古いカードを前面にする。線を下から上へ通る間に
      // 新しい順から古い順へ前面が移っていくので、通り過ぎた後もその続きの状態にしておく。
      if (line >= rect.bottom) {
        setTouchFocused(count - 1);
        return;
      }
      setTouchFocused(Math.min(count - 1, Math.floor(((line - rect.top) / rect.height) * count)));
    };
    const schedule = () => {
      if (frame === null) frame = window.requestAnimationFrame(update);
    };
    update();
    // 右ペインは独自にスクロールするので、どの要素のスクロールも拾う（capture）。
    window.addEventListener("scroll", schedule, { passive: true, capture: true });
    window.addEventListener("resize", schedule, { passive: true });
    return () => {
      window.removeEventListener("scroll", schedule, { capture: true });
      window.removeEventListener("resize", schedule);
      if (frame !== null) window.cancelAnimationFrame(frame);
    };
  }, [noHover, count]);

  const front = Math.min(count - 1, (noHover ? touchFocused : hovered) ?? 0);
  // 前面から遠い順（同じ距離なら上のカードを先）に並べる。最後が前面。
  const paintOrder = keys
    .map((key, index) => ({ key, index }))
    .sort((a, b) => Math.abs(b.index - front) - Math.abs(a.index - front) || a.index - b.index);

  return (
    <Tag
      {...containerProps}
      ref={ref as Ref<HTMLDivElement & HTMLLIElement>}
      className={[styles.stack, className].filter(Boolean).join(" ")}
      style={{ paddingBottom: (count - 1) * STACK_OFFSET_PX }}
    >
      {paintOrder.map(({ key, index }) => (
        <div
          key={key}
          className={styles.stackCard}
          style={{ top: index * STACK_OFFSET_PX }}
          onMouseEnter={() => setHovered(index)}
        >
          {renderCard(index)}
        </div>
      ))}
    </Tag>
  );
}
