import { describe, expect, it } from "vitest";
import { currentUtcOffsetMinutes, localTimeSlotKey, msUntilNextTimeSlot } from "./reactionTimeSlot";

// テストは実行環境のタイムゾーンに依存しないよう、現地時刻で組み立てた Date だけを使う。
function local(h: number, m = 0, s = 0, ms = 0) {
  return new Date(2026, 10, 10, h, m, s, ms);
}

describe("currentUtcOffsetMinutes", () => {
  it("getTimezoneOffset の符号を反転した値を返す", () => {
    const now = local(12);
    expect(currentUtcOffsetMinutes(now)).toBe(-now.getTimezoneOffset() || 0);
    expect(Object.is(currentUtcOffsetMinutes(now), -0)).toBe(false);
  });
});

describe("localTimeSlotKey", () => {
  it("現地の0時から4時間ずつ区切る", () => {
    expect(localTimeSlotKey(local(0))).toBe(localTimeSlotKey(local(3, 59, 59)));
    expect(localTimeSlotKey(local(3, 59, 59))).not.toBe(localTimeSlotKey(local(4)));
    expect(localTimeSlotKey(local(20))).toBe(localTimeSlotKey(local(23, 59)));
  });
});

describe("msUntilNextTimeSlot", () => {
  it("次の境目までの時間を返す", () => {
    expect(msUntilNextTimeSlot(local(3, 59, 0))).toBe(60_000);
    expect(msUntilNextTimeSlot(local(4))).toBe(4 * 3600_000);
  });

  it("最後の時間帯からは翌日の0時まで", () => {
    expect(msUntilNextTimeSlot(local(23, 30))).toBe(30 * 60_000);
  });
});
