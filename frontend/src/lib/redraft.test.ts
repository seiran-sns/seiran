import { describe, expect, it } from "vitest";
import type { Note } from "../api/client";
import { buildRedraftDraft, redraftBlockReason, redraftDraftTarget } from "./redraft";

function makeNote(overrides: Partial<Note> = {}): Note {
  return {
    id: "100",
    text: "こんにちは",
    createdAt: "2026-10-21T00:00:00Z",
    user: { id: 1, username: "seiran1", actorType: "local" },
    attachments: [],
    replyCount: 0,
    quoteCount: 0,
    repostCount: 0,
    linkCards: [],
    replyFediAllowed: true,
    replyBskyAllowed: true,
    replyBlocked: false,
    quoteBlocked: false,
    ...overrides,
  } as Note;
}

const attachment = {
  id: "7",
  url: "https://media.example/a.png",
  mimeType: "image/png",
  width: 10,
  height: 20,
  isSensitive: false,
  isGif: false,
  isAnimatedImage: false,
};

describe("redraftBlockReason", () => {
  it("通常のポストは選べる", () => {
    expect(redraftBlockReason(makeNote())).toBeNull();
  });

  it("返信元が無い返信は選べない", () => {
    expect(redraftBlockReason(makeNote({ replyStatus: "gone" }))).toBe("replyUnavailable");
    expect(redraftBlockReason(makeNote({ replyStatus: "pending" }))).toBe("replyUnavailable");
  });

  it("引用元が無い引用は選べない", () => {
    expect(redraftBlockReason(makeNote({ quoteStatus: "gone" }))).toBe("quoteUnavailable");
  });

  it("返信かつ引用のポストは選べない", () => {
    expect(redraftBlockReason(makeNote({ replyId: "1", quoteId: "2" }))).toBe("replyAndQuote");
  });

  it("idの無い添付があると選べない", () => {
    const remote = { ...attachment, id: undefined };
    expect(redraftBlockReason(makeNote({ attachments: [remote] }))).toBe("attachmentUnavailable");
  });
});

describe("redraftDraftTarget", () => {
  it("返信は返信元、引用は引用元をキーにし、それ以外は新規投稿の下書き", () => {
    expect(redraftDraftTarget(makeNote({ replyId: "5" }), 1)).toEqual({
      mode: "reply",
      userId: 1,
      postId: "5",
    });
    expect(redraftDraftTarget(makeNote({ quoteId: "6" }), 1)).toEqual({
      mode: "quote",
      userId: 1,
      postId: "6",
    });
    expect(redraftDraftTarget(makeNote(), 1)).toEqual({ mode: "compose", userId: 1 });
  });
});

describe("buildRedraftDraft", () => {
  it("本文・CW・配送先・公開範囲・添付を引き継ぐ", () => {
    const draft = buildRedraftDraft(
      makeNote({
        contentWarning: "注意",
        deliverFedi: true,
        deliverBsky: false,
        visibility: "unlisted",
        attachments: [attachment],
      }),
    );
    expect(draft.text).toBe("こんにちは");
    expect(draft.cwEnabled).toBe(true);
    expect(draft.cwGuide).toBe("注意");
    expect(draft.deliverBsky).toBe(false);
    expect(draft.visibility).toBe("unlisted");
    expect(draft.attachments).toHaveLength(1);
    expect(draft.attachments[0].id).toBe("7");
  });

  it("公開範囲が省略（public）または未対応値ならpublic", () => {
    expect(buildRedraftDraft(makeNote()).visibility).toBe("public");
  });

  it("期限が未来のアンケートは選択肢と期限を引き継ぐ", () => {
    const now = new Date("2026-10-21T00:00:00Z");
    const draft = buildRedraftDraft(
      makeNote({
        poll: {
          multiple: true,
          options: [
            { name: "A", votes: 3 },
            { name: "B", votes: 1 },
          ],
          endTime: "2026-10-22T00:00:00Z",
        },
      }),
      now,
    );
    expect(draft.pollEnabled).toBe(true);
    expect(draft.pollChoices).toEqual(["A", "B"]);
    expect(draft.pollMultiple).toBe(true);
    expect(draft.pollExpiry.kind).toBe("at");
  });

  it("期限が過ぎたアンケートは期限なしに戻す", () => {
    const now = new Date("2026-10-21T00:00:00Z");
    const draft = buildRedraftDraft(
      makeNote({
        poll: {
          multiple: false,
          options: [
            { name: "A", votes: 0 },
            { name: "B", votes: 0 },
          ],
          endTime: "2026-10-20T00:00:00Z",
        },
      }),
      now,
    );
    expect(draft.pollExpiry).toEqual({ kind: "none" });
  });
});
