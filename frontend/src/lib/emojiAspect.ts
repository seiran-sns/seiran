export type EmojiSpan = 1 | 2 | 3 | 4;

/**
 * span数ごとの画像幅(em, .itemのfont-size基準)。.itemのfont-sizeは1.25rem(20px)固定なので、
 * .itemの左右padding3pxは0.15emに相当する。全角(span1)=1.5emを基準に、spanが1増えるごとに
 * 画像は1.5em伸びるが、内側の隣接paddingは共有されるため実際の増分は1.5em+0.3em(左右padding
 * 0.15em×2) = 1.8em。よって width(span) = 1.8 * span - 0.3 (em) で 1.5 / 3.3 / 5.1 / 6.9em
 * （= 30 / 66 / 102 / 138px）となる。
 */
export function emojiSpanWidthEm(span: EmojiSpan): number {
  return 1.8 * span - 0.3;
}

/**
 * width/height比から絵文字ピッカーのグリッドセル幅（1〜4カラム分）を判定する。
 * しきい値は各spanのmax-width(px)自体の縦横比（66/30=2.2, 102/30=3.4, 138/30=4.6）。
 */
export function emojiAspectSpan(width?: number, height?: number): EmojiSpan {
  if (!width || !height) return 1;
  const ratio = width / height;
  if (ratio >= 4.6) return 4;
  if (ratio >= 3.4) return 3;
  if (ratio >= 2.2) return 2;
  return 1;
}
