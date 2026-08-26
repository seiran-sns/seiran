export type EmojiSpan = 1 | 2 | 3 | 4;

/** width/height比から絵文字ピッカーのグリッドセル幅（1〜4カラム分）を判定する。 */
export function emojiAspectSpan(width?: number, height?: number): EmojiSpan {
  if (!width || !height) return 1;
  const ratio = width / height;
  if (ratio < 2) return 1;
  if (ratio < 3) return 2;
  if (ratio < 4) return 3;
  return 4;
}
