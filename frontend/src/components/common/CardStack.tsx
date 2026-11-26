import { HTMLAttributes, ReactNode, Ref, useEffect, useLayoutEffect, useRef, useState } from "react";
import styles from "./CardStack.module.css";

/** 重ねたカードを縦にずらす量（px）。 */
const STACK_OFFSET_PX = 8;
/** タッチ環境で前面に出すカードを決める線の位置（画面の高さに対する割合、上から）。 */
const TOUCH_FOCUS_LINE = 0.25;
/** 広げる・畳むアニメーションの長さ（ms）。 */
const SPREAD_ANIMATION_MS = 200;

const easeOutCubic = (t: number) => 1 - (1 - t) ** 3;

function prefersReducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

/**
 * `el` をスクロールさせている一番近い祖先（右ペインの `.rightScroll` 等）。無ければ null（window）。
 * body まで来たら window とする: body は `overflow-x: hidden`（index.css）のため computed の
 * `overflow-y` が `auto` になるが、実際にスクロールするのは window で、body の scrollTop は動かない。
 * 狭い画面の `.rightScroll` のように overflow-y が auto でも伸びるだけでスクロールしない要素も飛ばす。
 */
function scrollParentOf(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p && p !== document.body && p !== document.documentElement; p = p.parentElement) {
    const { overflowY } = getComputedStyle(p);
    if ((overflowY === "auto" || overflowY === "scroll") && p.scrollHeight > p.clientHeight) return p;
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

/**
 * 畳んだまとまりのどのカードに線がかかっているか（`y` はまとまりの上端からの線の位置、`height` は
 * まとまりの高さ、`cardHeight` はカード1枚の高さ）。先頭と末尾のカードにまとまりの高さの半分
 * （ただしカード1枚の高さまで）ずつを割り当て、残りを中ほどのカードで等分する（残りが無ければ
 * 中ほどのカードにはかからない）。畳んだまとまりは帯が細いので、等分すると先頭・末尾に線が
 * かかる幅が狭すぎる。
 */
export function collapsedStackIndexAt(y: number, height: number, cardHeight: number, count: number): number {
  if (count <= 1) return 0;
  const end = Math.min(height / 2, Math.max(0, cardHeight));
  if (y < end) return 0;
  if (y >= height - end || count === 2) return count - 1;
  const middle = count - 2;
  const share = (height - 2 * end) / middle;
  return Math.min(count - 2, 1 + Math.floor((y - end) / share));
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
 * `TOUCH_FOCUS_LINE` の線にかかっているカードで、まとまり全体が線より上にあれば一番古いカード。
 * どちらでもなければ一番新しいカード。タッチ環境で線にかかっているカードは、広げないまとまり
 * （タイムライン）ではまとまりの高さを件数で等分して決める。広げるまとまり（`spreadOffsetPx`）では
 * 畳んでいれば `collapsedStackIndexAt`、広げていればマウス環境と同じく線の位置に見えているカード。
 *
 * 上下関係は前面のカードに近いほど上にする（1〜5 で 3 が前面なら 3 > 2 = 4 > 1 = 5）。前面より
 * 上のカードは上端の帯、下のカードは下端の帯が見えるので、全部のカードの帯が見える。
 * z-index は使わず、遠いカードほど DOM の前に置いて描画順で重ねる。z-index を付けるとカードごとに
 * 重なりの文脈ができ、中のホバープレビュー（position: fixed）が左ペイン等の下に潜る。同じ理由で
 * ずらしも transform ではなく top で行う（transform の中では position: fixed の基準がカードになる）。
 *
 * カードは一番高いカードの高さまで伸ばす（`.stackCard` の直下の要素）。
 *
 * `spreadOffsetPx` を渡すと、ずらし幅をその値に広げる（通知欄）。広げる・畳むときはずらし幅を
 * `SPREAD_ANIMATION_MS` かけて変え、その間ずっと基準のカードが画面上で動かないようスクロールで
 * 追いかける。
 * - マウス環境: マウスが乗っている間だけ広げる。乗った瞬間のカードを基準にし、まとまりより上を
 *   上へ送って、そのカードを中心に上下へ広がって見えるようにする（ポインタの下のカードが
 *   入れ替わらない）。広げたまま押す（カードかその中のリンクをクリックする）と広げた状態を
 *   「固定」し、ほかの画面から戻ってきても広げたままにする。固定はもう一度マウスが乗るか、
 *   まとまりが画面の外へ出ると解ける。固定中は `pinnedClassName` を付ける（呼び出し側が枠線で囲う）。
 * - タッチ環境: `TOUCH_FOCUS_LINE` の線がかかっている間だけ広げる。線にかかったカードを基準に
 *   広げ、線が下へ抜けて畳むときはまとまりの下端を基準にする（線の下の一覧がずれない）。戻って
 *   きたときはスクロール位置が戻れば線がまた同じところにかかるので固定はせず、広げていたことだけ
 *   覚えておいて広げた形で出す（畳んだ形で出してから広げると、戻したスクロール位置がずれる）。
 *
 * `initialFront`・`initialSpread` は前回の前面のカード（キー）と、覚えておく広げた状態（マウス環境は
 * 固定、タッチ環境は線がかかって広げていたか）。変わったときは `onFrontChange`・`onSpreadChange` で
 * 知らせるので、呼び出し側が覚えておいて次に渡す。
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
  initialSpread = false,
  onSpreadChange,
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
  initialSpread?: boolean;
  onSpreadChange?: (spread: boolean) => void;
  pinnedClassName?: string;
}) {
  const ref = useRef<HTMLElement>(null);
  const noHover = useNoHover();
  const [hoveredKey, setHoveredKey] = useState<string | null>(initialFront ?? null);
  const [touchFocused, setTouchFocused] = useState<number | null>(null);
  const [mouseInside, setMouseInside] = useState(false);
  const [pinned, setPinned] = useState(() => initialSpread && !noHover);
  const [touchSpread, setTouchSpread] = useState(() => initialSpread && noHover);
  const touchSpreadRef = useRef(touchSpread);
  const count = keys.length;
  const hoveredIndex = hoveredKey === null ? -1 : keys.indexOf(hoveredKey);
  const hovered = hoveredIndex < 0 ? null : hoveredIndex;

  const canSpread = spreadOffsetPx !== undefined;
  const hoverSpreadable = canSpread && !noHover;
  const spread = canSpread && (noHover ? touchSpread : mouseInside || pinned);
  const targetOffset = spread ? (spreadOffsetPx ?? STACK_OFFSET_PX) : STACK_OFFSET_PX;

  const changePinned = (next: boolean) => {
    setPinned(next);
    onSpreadChange?.(next);
  };

  // 広げる・畳む間の基準（乗ったカード・線にかかったカードの上端、まとまりの下端）。広げる・畳む
  // 状態を変える直前に置き、アニメーションの間ずっとこれが元の画面内の位置に来るようスクロールで戻す。
  const anchorRef = useRef<SpreadAnchor | null>(null);
  const setAnchor = (measure: () => number | undefined) => {
    const value = measure();
    anchorRef.current = value === undefined ? null : { measure, value };
  };

  // ずらし幅を目標へアニメーションさせる（top と padding-bottom を毎フレーム変える。CSS の
  // transition にしないのは、フレームごとに基準を合わせるスクロールを同じフレームで行うため）。
  const [offset, setOffset] = useState(targetOffset);
  const offsetRef = useRef(targetOffset);
  useEffect(() => {
    const from = offsetRef.current;
    if (from === targetOffset) {
      anchorRef.current = null;
      return;
    }
    if (prefersReducedMotion()) {
      offsetRef.current = targetOffset;
      setOffset(targetOffset);
      return;
    }
    const start = performance.now();
    let frame = window.requestAnimationFrame(function step(now) {
      const t = Math.min(1, (now - start) / SPREAD_ANIMATION_MS);
      const value = t >= 1 ? targetOffset : from + (targetOffset - from) * easeOutCubic(t);
      offsetRef.current = value;
      setOffset(value);
      if (t < 1) frame = window.requestAnimationFrame(step);
    });
    return () => window.cancelAnimationFrame(frame);
  }, [targetOffset]);

  // ずらし幅を変えたフレームで、基準が元の画面内の位置に来るようスクロールで戻す。終わったら基準を
  // 捨てるが、ブラウザのスクロールアンカーが後から動かすこともあるので次のフレームでもう一度合わせる。
  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    const el = ref.current;
    if (!anchor || !el) return;
    const scroller = scrollParentOf(el);
    const correct = () => {
      const now = anchor.measure();
      if (now !== undefined) scrollByPx(scroller, now - anchor.value);
    };
    correct();
    if (offset !== targetOffset) return;
    anchorRef.current = null;
    const frame = window.requestAnimationFrame(correct);
    return () => window.cancelAnimationFrame(frame);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [offset]);

  // 固定したまとまりが画面の外へ出たら固定を解く。戻ってきた直後はスクロール位置を合わせる前で
  // 画面外にあることがあるので、一度画面に入ってから出たときだけ解く。画面より上へ出たときは、
  // 畳んで縮んだ分だけ下の一覧が上へずれないよう、まとまりの下端を基準にする。
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
      if (entry.boundingClientRect.bottom <= rootTop) setAnchor(() => ref.current?.getBoundingClientRect().bottom);
      setPinned(false);
      onSpreadChange?.(false);
    });
    observer.observe(el);
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pinned]);

  useEffect(() => {
    if (!noHover) return;
    let frame: number | null = null;
    // 広げた形で戻ってきたときは、スクロール位置を合わせる前で線がかかっていないことがあるので、
    // 一度線がかかるまでは畳まない。
    let seen = !touchSpreadRef.current;
    const changeTouchSpread = (next: boolean) => {
      touchSpreadRef.current = next;
      setTouchSpread(next);
      onSpreadChange?.(next);
    };
    const update = () => {
      frame = null;
      const el = ref.current;
      if (!el) return;
      const rect = el.getBoundingClientRect();
      const line = window.innerHeight * TOUCH_FOCUS_LINE;
      let focused: number | null;
      if (line < rect.top || rect.height <= 0) {
        focused = null;
      } else if (line >= rect.bottom) {
        // 線より上へ通り過ぎたまとまりは一番古いカードを前面にする。線を下から上へ通る間に
        // 新しい順から古い順へ前面が移っていくので、通り過ぎた後もその続きの状態にしておく。
        focused = count - 1;
      } else if (touchSpreadRef.current) {
        // 広げている間はマウス環境と同じく、線の位置に見えているカード（重なりの一番上）。
        const hit = document
          .elementFromPoint(rect.left + rect.width / 2, line)
          ?.closest<HTMLElement>("[data-stack-index]");
        const index = hit && el.contains(hit) ? Number(hit.dataset.stackIndex) : NaN;
        focused = Number.isNaN(index) ? null : index;
      } else if (canSpread) {
        const cardHeight = rect.height - (count - 1) * offsetRef.current;
        focused = collapsedStackIndexAt(line - rect.top, rect.height, cardHeight, count);
      } else {
        // 広げないまとまり（タイムライン）は高さを件数で等分する。
        focused = Math.min(count - 1, Math.floor(((line - rect.top) / rect.height) * count));
      }
      // 広げている間に線の位置のカードが取れない（隙間・描き直し中）ときは前面を変えない。
      if (focused !== null || !touchSpreadRef.current || line < rect.top) setTouchFocused(focused);
      if (!canSpread) return;
      const inside = rect.height > 0 && rect.top <= line && line < rect.bottom;
      if (inside) seen = true;
      if (inside === touchSpreadRef.current || (!inside && !seen)) return;
      if (inside && focused !== null) {
        const card = el.querySelector<HTMLElement>(`[data-stack-index="${focused}"]`);
        setAnchor(() => card?.getBoundingClientRect().top);
      } else if (!inside && line >= rect.bottom) {
        setAnchor(() => ref.current?.getBoundingClientRect().bottom);
      }
      changeTouchSpread(inside);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [noHover, count, canSpread]);

  const front = Math.min(count - 1, (noHover ? touchFocused : hovered) ?? 0);
  const showPinned = hoverSpreadable && pinned && !mouseInside;
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
        hoverSpreadable
          ? () => {
              if (pinned) changePinned(false);
            }
          : undefined
      }
      onMouseLeave={hoverSpreadable ? () => setMouseInside(false) : undefined}
      onClickCapture={
        hoverSpreadable
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
          data-stack-index={index}
          onMouseEnter={(e) => {
            setHoveredKey(key);
            onFrontChange?.(key);
            if (!hoverSpreadable) return;
            if (!spread) {
              const card = e.currentTarget;
              setAnchor(() => card.getBoundingClientRect().top);
            }
            setMouseInside(true);
          }}
        >
          {renderCard(index)}
        </div>
      ))}
    </Tag>
  );
}
