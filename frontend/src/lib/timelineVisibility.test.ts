import { describe, expect, it } from "vitest";
import type { Note } from "../api/client";
import { filterTimelineNotes } from "./timelineVisibility";

function note(id: string, visibility: Note["visibility"]): Note {
  return {
    id,
    text: id,
    createdAt: "",
    user: { id: 1, username: "me", actorType: "local" },
    attachments: [],
    visibility,
    replyFediAllowed: true,
    replyBskyAllowed: true,
    replyBlocked: false,
    quoteBlocked: false,
    replyCount: 0,
    quoteCount: 0,
    repostCount: 0,
    linkCards: [],
  };
}

const notes = [
  note("public", "public"),
  note("unlisted", "unlisted"),
  note("followers", "followers_only"),
];

describe("filterTimelineNotes", () => {
  it.each(["local", "global"] as const)(
    "%sでは本人の投稿かどうかに関係なくひかえめ・プライベートを除外する",
    (kind) => {
      expect(filterTimelineNotes({ kind }, notes).map((item) => item.id)).toEqual(["public"]);
    }
  );

  it.each(["home", "social"] as const)("%sではひかえめ・プライベートを保持する", (kind) => {
    expect(filterTimelineNotes({ kind }, notes)).toEqual(notes);
  });
});
