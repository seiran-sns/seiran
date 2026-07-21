import { describe, expect, it } from "vitest";
import type { Note } from "../../api/client";
import { replyVisibilityConstraint } from "./PostComposer";

function makeNote(visibility?: string): Note {
  return {
    id: "1",
    text: "hello",
    createdAt: "2026-07-21T00:00:00Z",
    user: { id: 1, username: "alice", actorType: "local" },
    attachments: [],
    visibility,
  };
}

describe("replyVisibilityConstraint", () => {
  it("返信先が無い場合は制約なし（public）", () => {
    expect(replyVisibilityConstraint(undefined)).toEqual({ forced: null, defaultValue: "public" });
  });

  it("返信先がfollowers_onlyの場合はfollowers_onlyに強制する", () => {
    expect(replyVisibilityConstraint(makeNote("followers_only"))).toEqual({
      forced: "followers_only",
      defaultValue: "followers_only",
    });
  });

  it("返信先がunlistedの場合はデフォルトunlistedだが強制はしない", () => {
    expect(replyVisibilityConstraint(makeNote("unlisted"))).toEqual({ forced: null, defaultValue: "unlisted" });
  });

  it("返信先がpublic(未指定)の場合は制約なし", () => {
    expect(replyVisibilityConstraint(makeNote(undefined))).toEqual({ forced: null, defaultValue: "public" });
  });

  it("返信先がdirectの場合も制約なし（public扱い）", () => {
    expect(replyVisibilityConstraint(makeNote("direct"))).toEqual({ forced: null, defaultValue: "public" });
  });

  it("想定外の値でも制約なしにフォールバックする", () => {
    expect(replyVisibilityConstraint(makeNote("something_unexpected"))).toEqual({ forced: null, defaultValue: "public" });
  });
});
