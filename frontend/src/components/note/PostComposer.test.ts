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
    replyFediAllowed: true,
    replyBskyAllowed: true,
    replyCount: 0,
    quoteCount: 0,
    repostCount: 0,
    linkCards: [],
  };
}

describe("replyVisibilityConstraint", () => {
  it("返信先が無い場合は3段階すべて選択可（デフォルトpublic）", () => {
    expect(replyVisibilityConstraint(undefined)).toEqual({
      options: ["public", "unlisted", "followers_only"],
      defaultValue: "public",
    });
  });

  it("返信先がfollowers_onlyの場合はfollowers_onlyのみ（これ以上狭められない）", () => {
    expect(replyVisibilityConstraint(makeNote("followers_only"))).toEqual({
      options: ["followers_only"],
      defaultValue: "followers_only",
    });
  });

  it("返信先がunlistedの場合はunlisted/followers_only（狭める方向のみ、publicは選べない）", () => {
    expect(replyVisibilityConstraint(makeNote("unlisted"))).toEqual({
      options: ["unlisted", "followers_only"],
      defaultValue: "unlisted",
    });
  });

  it("返信先がpublic(未指定)の場合は3段階すべて選択可", () => {
    expect(replyVisibilityConstraint(makeNote(undefined))).toEqual({
      options: ["public", "unlisted", "followers_only"],
      defaultValue: "public",
    });
  });

  it("返信先がdirectの場合も制約なし（public扱い）", () => {
    expect(replyVisibilityConstraint(makeNote("direct"))).toEqual({
      options: ["public", "unlisted", "followers_only"],
      defaultValue: "public",
    });
  });

  it("想定外の値でも制約なしにフォールバックする", () => {
    expect(replyVisibilityConstraint(makeNote("something_unexpected"))).toEqual({
      options: ["public", "unlisted", "followers_only"],
      defaultValue: "public",
    });
  });
});
