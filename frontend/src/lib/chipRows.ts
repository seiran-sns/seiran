/**
 * 折り返して並ぶチップ（`flex-wrap: wrap`）を `maxLines` 行に収めるとき、先頭から何個見せるか。
 *
 * 全部が `maxLines` 行に収まれば `overflow: false` で全部を見せる。収まらなければ、最後の行の末尾に
 * 幅 `buttonWidth` のボタン（「全部」）を置ける分だけ最後の行のチップを減らす。
 * 幅は実測値（小数）で、行の詰め方はブラウザの flex の折り返しと同じく先頭から貪欲に詰める。
 */
export function fitChipsInLines(
  widths: number[],
  containerWidth: number,
  gap: number,
  buttonWidth: number,
  maxLines: number
): { visible: number; overflow: boolean } {
  // 実測の丸め誤差で、ちょうど収まるチップが次の行へ回ったと判定しないための余裕。
  const EPS = 0.5;
  const fits = (x: number, w: number) => (x === 0 ? w : x + gap + w) <= containerWidth + EPS;

  // 各チップの行番号（0始まり）と、その行の右端の位置。
  let line = 0;
  let x = 0;
  const ends: { line: number; end: number }[] = [];
  for (const w of widths) {
    if (x > 0 && !fits(x, w)) {
      line += 1;
      x = 0;
    }
    x = x === 0 ? w : x + gap + w;
    ends.push({ line, end: x });
  }
  if (line < maxLines) return { visible: widths.length, overflow: false };

  // 最後の行に入るチップのうち、末尾にボタンを置ける所まで。
  let visible = ends.findIndex((e) => e.line >= maxLines);
  while (visible > 0) {
    const last = ends[visible - 1];
    if (last.line < maxLines - 1 || fits(last.end, buttonWidth)) break;
    visible -= 1;
  }
  return { visible, overflow: true };
}
