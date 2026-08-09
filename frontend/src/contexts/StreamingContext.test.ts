import { describe, expect, it, vi } from "vitest";
import type { Note } from "../api/client";
import { resolveStreamNote } from "./resolveStreamNote";

const streamed = {
  id: "123",
  text: "引用ポスト",
  createdAt: "2026-07-28T00:00:00Z",
  user: { id: 1, username: "alice", actorType: "fedi" },
  attachments: [],
  replyCount: 0,
  quoteCount: 0,
  repostCount: 0,
  linkCards: [],
};

describe("resolveStreamNote", () => {
  it("通常APIの完全な引用・アンケート・添付データでストリーム投稿を補完する", async () => {
    const complete = {
      ...streamed,
      quoteId: "100",
      quote: { ...streamed, id: "100", text: "引用元" },
      poll: { multiple: false, options: [{ name: "はい", votes: 1 }] },
      attachments: [{
        url: "https://example.com/image.jpg",
        mimeType: "image/jpeg",
        width: 640,
        height: 480,
        isSensitive: false,
        isGif: false,
      }],
    } satisfies Note;
    const fetchNote = vi.fn().mockResolvedValue(complete);

    await expect(resolveStreamNote(streamed, fetchNote)).resolves.toEqual(complete);
    expect(fetchNote).toHaveBeenCalledWith("123");
  });

  it("通常APIの取得に失敗した場合はストリーム投稿をそのまま表示する", async () => {
    const fetchNote = vi.fn().mockRejectedValue(new Error("offline"));

    await expect(resolveStreamNote(streamed, fetchNote)).resolves.toMatchObject({
      id: "123",
      text: "引用ポスト",
      attachments: [],
    });
  });
});
