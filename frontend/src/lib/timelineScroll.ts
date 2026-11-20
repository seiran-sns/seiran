/**
 * タイムラインのスクロール位置を、画素値ではなく「どのポストが一番上に見えているか」で表す。
 *
 * `window.scrollY` で覚えると、画面より上のポストが伸び縮みしたり（リアクション・画像の
 * 読み込み）上に新しいポストが入ったりしただけで、見ていた位置がずれる。一番上に見えている
 * ポストと、その上端の画面内の位置を覚えておけば、上の高さが変わってもそのポストを同じ位置へ
 * 戻せる。
 *
 * ポストの要素には `data-timeline-id` を付ける（`NoteList`）。
 */
export type TimelineScroll =
  /** 先頭のポストが上端まで見えている。`y` は一覧より上（投稿欄等）のスクロール量。 */
  | { kind: "top"; y: number }
  /** `id` のポストが一番上に見えていて、その上端が画面内の `top`（px、負なら見切れ）にある。 */
  | { kind: "anchor"; id: string; top: number };

export const TIMELINE_TOP: TimelineScroll = { kind: "top", y: 0 };

function timelineItems(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>("[data-timeline-id]"));
}

/**
 * 今の位置を読み取る。`coverBottom` は上端に張り付いて一覧を隠している要素（タブ）の下端で、
 * それより上に隠れたポストは見えていないものとして扱う。
 */
export function captureTimelineScroll(container: HTMLElement | null, coverBottom = 0): TimelineScroll {
  const items = container ? timelineItems(container) : [];
  if (items.length === 0) return { kind: "top", y: window.scrollY };
  const line = Math.max(0, coverBottom);
  // ポストは文書順に縦に並ぶので、下端が line より下にある最初のポストを二分探索で探す。
  let lo = 0;
  let hi = items.length - 1;
  let found = items.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (items[mid].getBoundingClientRect().bottom > line) {
      found = mid;
      hi = mid - 1;
    } else {
      lo = mid + 1;
    }
  }
  const el = items[found];
  const top = el.getBoundingClientRect().top;
  if (found === 0 && top >= line - 1) return { kind: "top", y: window.scrollY };
  return { kind: "anchor", id: el.dataset.timelineId ?? "", top };
}

/**
 * 覚えた位置へ戻す。`anchor` のポストが一覧に無い（削除された等）ときは何もせず `false`。
 * ずれが1px未満なら動かさない（スクロールのたびに自分の補正で補正を呼ばないため）。
 */
export function applyTimelineScroll(container: HTMLElement | null, scroll: TimelineScroll): boolean {
  if (scroll.kind === "top") {
    window.scrollTo(0, scroll.y);
    return true;
  }
  const el = container?.querySelector<HTMLElement>(`[data-timeline-id="${CSS.escape(scroll.id)}"]`);
  if (!el) return false;
  const delta = el.getBoundingClientRect().top - scroll.top;
  if (Math.abs(delta) >= 1) window.scrollTo(0, window.scrollY + delta);
  return true;
}
