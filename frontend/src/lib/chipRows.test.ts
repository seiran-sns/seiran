import { describe, expect, it } from "vitest";
import { fitChipsInLines } from "./chipRows";

describe("fitChipsInLines", () => {
  it("2行に収まれば全部見せる", () => {
    // 幅100・間隔10: 1行に 30+10+30+10+30 = 110 は入らないので 30,30 / 30,30 の2行。
    expect(fitChipsInLines([30, 30, 30, 30], 100, 10, 20, 2)).toEqual({ visible: 4, overflow: false });
  });

  it("2行を超えたら2行目の末尾にボタンが入る所まで減らす", () => {
    // 30,30 / 30,30 / 30 の3行。2行目は 30+10+30 = 70 で、ボタン20を足すと 100 で入る。
    expect(fitChipsInLines([30, 30, 30, 30, 30], 100, 10, 20, 2)).toEqual({ visible: 4, overflow: true });
    // ボタン25なら入らないので2行目のチップを1つ減らす。
    expect(fitChipsInLines([30, 30, 30, 30, 30], 100, 10, 25, 2)).toEqual({ visible: 3, overflow: true });
  });

  it("1行目より前には戻らない", () => {
    // 1個で1行を占めるチップばかりなら、2行目のチップを外してボタンだけを置く。
    expect(fitChipsInLines([100, 100, 100], 100, 10, 20, 2)).toEqual({ visible: 1, overflow: true });
  });

  it("ちょうど収まる幅は丸め誤差があっても同じ行とみなす", () => {
    expect(fitChipsInLines([45.3, 45.2, 45, 45], 100.2, 10, 20, 2).overflow).toBe(false);
  });
});
