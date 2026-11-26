import { HTMLAttributes, ReactNode, Ref, useEffect, useLayoutEffect, useRef, useState } from "react";
import styles from "./CardStack.module.css";

/** 重ねたカードを縦にずらす量（px）。 */
const STACK_OFFSET_PX = 8;
/** タッチ環境で前面に出すカードを決める線の位置（画面の高さに対する割合、上から）。 */
const TOUCH_FOCUS_LINE = 0.25;

/** `el` をスクロールさせている一番近い祖先（右ペインの `.rightScroll` 等）。無ければ null（window）。 */
function scrollParentOf(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const { overflowY } = getComputedStyle(p);
    if (overflowY === "auto" || overflowY === "scroll") return p;
  }
  return null;
}

function scrollByPx(scroller: HTMLElement | null, delta: number) {
  if (Math.abs(delta) < 1) return;
  if (scroller) scroller.scrollTop += delta;
  else window.scrollTo(0, window.scrollY + delta);
}

/** 広げる・畳むときに画面上の位置を動かさない基準。`measure` が今の画面内の位置を返す。 */
interface SpreadAnchor {
  measure: () => number | undefined;
  value: number;
}

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
 *
 * `spreadOffsetPx` を渡すと、マウス環境ではマウスが乗っている間だけずらし幅をその値に広げる（通知欄）。
 * 乗った瞬間のカードを画面上で動かさないよう、まとまりより上を上へ送る分だけスクロールし、
 * そのカードを中心に上下へ広がって見えるようにする。広げたまま押す（カードかその中のリンクを
 * クリックする）と広げた状態を「固定」し、ほかの画面から戻ってきても広げたままにする。固定は
 * もう一度マウスが乗るか、まとまりが画面の外へ出ると解ける。固定中は `pinnedClassName` を付ける
 * （呼び出し側が枠線で囲う）。
 *
 * `initialFront`・`initialPinned` は前回の前面のカード（キー）と固定の状態。変わったときは
 * `onFrontChange`・`onPinnedChange` で知らせるので、呼び出し側が覚えておいて次に渡す。
 */
export default function CardStack({
  keys,
  renderCard,
  as: Tag = "div",
  className,
  containerProps,
  spreadOffsetPx,
  initialFront,
  onFrontChange,
  initialPinned = false,
  onPinnedChange,
  pinnedClassName,
}: {
  keys: string[];
  renderCard: (index: number) => ReactNode;
  as?: "div" | "li";
  className?: string;
  containerProps?: HTMLAttributes<HTMLElement> & Record<`data-${string}`, string>;
  spreadOffsetPx?: number;
  initialFront?: string;
  onFrontChange?: (key: string) => void;
  initialPinned?: boolean;
  onPinnedChange?: (pinned: boolean) => void;
  pinnedClassName?: string;
}) {
  const ref = useRef<HTMLElement>(null);
  const noHover = useNoHover();
  const [hoveredKey, setHoveredKey] = useState<string | null>(initialFront ?? null);
  const [touchFocused, setTouchFocused] = useState<number | null>(null);
  const [mouseInside, setMouseInside] = useState(false);
  const [pinned, setPinned] = useState(initialPinned);
  const count = keys.length;
  const hoveredIndex = hoveredKey === null ? -1 : keys.indexOf(hoveredKey);
  const hovered = hoveredIndex < 0 ? null : hoveredIndex;

  const spreadable = spreadOffsetPx !== undefined && !noHover;
  const spread = spreadable && (mouseInside || pinned);
  const offset = spread ? (spreadOffsetPx ?? STACK_OFFSET_PX) : STACK_OFFSET_PX;

  const changePinned = (next: boolean) => {
    setPinned(next);
    onPinnedChange?.(next);
  };

  // 広げた・畳んだ後、基準（乗ったカードの上端・画面より上へ出たまとまりの下端）が元の画面内の
  // 位置に来るようスクロールで戻す。ブラウザのスクロールアンカーが先に動かしていることもあるので、
  // 次のフレームでもう一度合わせる。
  const anchorRef = useRef<SpreadAnchor | null>(null);
  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    anchorRef.current = null;
    const el = ref.current;
    if (!anchor || !el) return;
    const scroller = scrollParentOf(el);
    const correct = () => {
      const now = anchor.measure();
      if (now !== undefined) scrollByPx(scroller, now - anchor.value);
    };
    correct();
    const frame = window.requestAnimationFrame(correct);
    return () => window.cancelAnimationFrame(frame);
  }, [spread]);

  // 固定したまとまりが画面の外へ出たら固定を解く。戻ってきた直後はスクロール位置を合わせる前で
  // 画面外にあることがあるので、一度画面に入ってから出たときだけ解く。画面より上へ出たときは、
  // 畳んで縮んだ分だけ下の一覧が上へずれないよう、まとまりの下端を基準に戻す。
  useEffect(() => {
    const el = ref.current;
    if (!pinned || !el || typeof IntersectionObserver === "undefined") return;
    let seen = false;
    const observer = new IntersectionObserver((entries) => {
      const entry = entries[entries.length - 1];
      if (entry.isIntersecting) {
        seen = true;
        return;
      }
      if (!seen) return;
      const rootTop = entry.rootBounds?.top ?? 0;
      if (entry.boundingClientRect.bottom <= rootTop) {
        anchorRef.current = {
          measure: () => ref.current?.getBoundingClientRect().bottom,
          value: entry.boundingClientRect.bottom,
        };
      }
      setPinned(false);
      onPinnedChange?.(false);
    });
    observer.observe(el);
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pinned]);

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
  const showPinned = spreadable && pinned && !mouseInside;
  // 前面から遠い順（同じ距離なら上のカードを先）に並べる。最後が前面。
  const paintOrder = keys
    .map((key, index) => ({ key, index }))
    .sort((a, b) => Math.abs(b.index - front) - Math.abs(a.index - front) || a.index - b.index);

  return (
    <Tag
      {...containerProps}
      ref={ref as Ref<HTMLDivElement & HTMLLIElement>}
      className={[styles.stack, className, showPinned && pinnedClassName].filter(Boolean).join(" ")}
      style={{ paddingBottom: (count - 1) * offset }}
      onMouseEnter={
        spreadable
          ? () => {
              if (pinned) changePinned(false);
            }
          : undefined
      }
      onMouseLeave={spreadable ? () => setMouseInside(false) : undefined}
      onClickCapture={
        spreadable
          ? () => {
              if (spread && !pinned) changePinned(true);
            }
          : undefined
      }
    >
      {paintOrder.map(({ key, index }) => (
        <div
          key={key}
          className={styles.stackCard}
          style={{ top: index * offset }}
          onMouseEnter={(e) => {
            setHoveredKey(key);
            onFrontChange?.(key);
            if (spreadable && !spread) {
              const card = e.currentTarget;
              anchorRef.current = { measure: () => card.getBoundingClientRect().top, value: card.getBoundingClientRect().top };
            }
            if (spreadable) setMouseInside(true);
          }}
        >
          {renderCard(index)}
        </div>
      ))}
    </Tag>
  );
}
