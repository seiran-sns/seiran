import { describe, expect, it } from "vitest";
import { ApiError } from "../api/core";
import { diffEdits, isEditConflict } from "./editDiff";

describe("diffEdits", () => {
  it("変えた項目だけを編集前・編集後の組にする", () => {
    expect(
      diffEdits(
        { name: "a", isPublic: false, tags: ["x"], capacity: null as number | null },
        { name: "b", isPublic: false, tags: ["x"], capacity: 10 as number | null },
      ),
    ).toEqual({ name: { from: "a", to: "b" }, capacity: { from: null, to: 10 } });
  });

  it("配列は中身で比べる", () => {
    expect(diffEdits({ tags: ["x"] }, { tags: ["x", "y"] })).toEqual({
      tags: { from: ["x"], to: ["x", "y"] },
    });
    expect(diffEdits({ tags: ["x"] }, { tags: ["x"] })).toEqual({});
  });
});

describe("isEditConflict", () => {
  it("EDIT_CONFLICT だけを衝突とみなす", () => {
    expect(isEditConflict(new ApiError("EDIT_CONFLICT", 409))).toBe(true);
    expect(isEditConflict(new ApiError("HAS_MEDIA_FILES", 409))).toBe(false);
    expect(isEditConflict(new Error("x"))).toBe(false);
  });
});
