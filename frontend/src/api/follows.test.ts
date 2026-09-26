import { describe, expect, it } from "vitest";
import { followTargetOf } from "./follows";

describe("followTargetOf", () => {
  it("actorIdがあればそれでフォロー対象を指定する", () => {
    expect(followTargetOf("123", "alice@example.com")).toEqual({ actorId: "123" });
  });

  it("actorIdが無ければtarget文字列で指定する", () => {
    expect(followTargetOf(undefined, "alice@example.com")).toEqual({ target: "alice@example.com" });
    expect(followTargetOf("", "alice")).toEqual({ target: "alice" });
  });
});
