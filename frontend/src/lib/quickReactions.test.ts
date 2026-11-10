import { describe, expect, it } from "vitest";
import type { FrequentReaction, PublicEmoji } from "../api/client";
import { isSameReaction, selectQuickReactions } from "./quickReactions";

function freq(content: string, count = 1): FrequentReaction {
  return { content, count, emojiUrl: null };
}

function custom(name: string, width: number, height: number): PublicEmoji {
  return {
    id: name,
    aliases: [],
    name,
    category: null,
    host: null,
    url: `https://example.test/${name}.png`,
    license: null,
    width,
    height,
    blurhash: "",
  };
}

describe("selectQuickReactions", () => {
  it("❤️ を先頭に、よく使う絵文字を頻度順に最大3つ並べる", () => {
    const result = selectQuickReactions(
      [freq("👍"), freq("🎉"), freq("😂"), freq("🙏")],
      []
    );
    expect(result.map((q) => q.content)).toEqual(["❤️", "👍", "🎉", "😂"]);
  });

  it("使ったことのある絵文字が3種類に満たなければその分だけ並べる", () => {
    expect(selectQuickReactions([], []).map((q) => q.content)).toEqual(["❤️"]);
    expect(selectQuickReactions([freq("👍")], []).map((q) => q.content)).toEqual(["❤️", "👍"]);
  });

  it("❤️ はよく使う絵文字の枠に重複させない（VS16 の有無も同一視する）", () => {
    const result = selectQuickReactions([freq("❤️"), freq("❤"), freq("👍")], []);
    expect(result.map((q) => q.content)).toEqual(["❤️", "👍"]);
  });

  it("登録済みのローカルカスタム絵文字のうち1マス幅のものだけを候補にする", () => {
    const result = selectQuickReactions(
      [
        freq(":wide@.:"),
        freq(":square@.:"),
        freq(":remote@other.example:"),
        freq(":deleted@.:"),
        freq(":legacy:"),
      ],
      [custom("wide", 300, 100), custom("square", 128, 128), custom("legacy", 200, 100)]
    );
    expect(result).toEqual([
      { content: "❤️" },
      { content: ":square:", imageUrl: "https://example.test/square.png" },
      { content: ":legacy:", imageUrl: "https://example.test/legacy.png" },
    ]);
  });
});

describe("isSameReaction", () => {
  it("ローカルカスタム絵文字は送信値と保存値の表記差を同一視する", () => {
    expect(isSameReaction(":blob:", ":blob@.:")).toBe(true);
    expect(isSameReaction(":blob:", ":blob@other.example:")).toBe(false);
    expect(isSameReaction(":blob@a.example:", ":blob@a.example:")).toBe(true);
    expect(isSameReaction(":blob:", ":blobcat:")).toBe(false);
  });

  it("Unicode 絵文字は VS16 の有無を無視して比べる", () => {
    expect(isSameReaction("❤️", "❤")).toBe(true);
    expect(isSameReaction("❤️", "👍")).toBe(false);
    expect(isSameReaction("👍", ":thumbsup:")).toBe(false);
  });
});
