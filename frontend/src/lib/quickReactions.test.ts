import { describe, expect, it } from "vitest";
import type { FrequentReaction, PublicEmoji } from "../api/client";
import { isSameReaction, mergeQuickReactionsStable, selectQuickReactions } from "./quickReactions";

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
  it("よく使う順のまま、数を絞らずに並べる", () => {
    const result = selectQuickReactions(
      [freq("👍"), freq("🎉"), freq("😂"), freq("🙏"), freq("👀")],
      []
    );
    expect(result.map((q) => q.content)).toEqual(["👍", "🎉", "😂", "🙏", "👀"]);
  });

  it("❤️ はボタンバーにあるので、上位でも並べない（VS16 の有無も同一視する）", () => {
    const result = selectQuickReactions([freq("❤️"), freq("👍"), freq("❤")], []);
    expect(result.map((q) => q.content)).toEqual(["👍"]);
  });

  it("使ったことのある絵文字が無ければ空", () => {
    expect(selectQuickReactions([], [])).toEqual([]);
    expect(selectQuickReactions([freq("❤️")], [])).toEqual([]);
  });

  it("登録済みのローカルカスタム絵文字は横長でも候補にし、寸法を添える", () => {
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
      { content: ":wide:", imageUrl: "https://example.test/wide.png", width: 300, height: 100 },
      { content: ":square:", imageUrl: "https://example.test/square.png", width: 128, height: 128 },
      { content: ":legacy:", imageUrl: "https://example.test/legacy.png", width: 200, height: 100 },
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

describe("mergeQuickReactionsStable", () => {
  const q = (content: string) => ({ content });

  it("候補に残る絵文字は頻度順が入れ替わっても位置を変えない", () => {
    expect(
      mergeQuickReactionsStable([q("👍"), q("🎉"), q("😂")], [q("🎉"), q("👍"), q("😂")])
    ).toEqual([q("👍"), q("🎉"), q("😂")]);
  });

  it("外れた絵文字の位置に新しく入った絵文字を入れる", () => {
    expect(
      mergeQuickReactionsStable([q("👍"), q("🎉"), q("😂")], [q("🙏"), q("👍"), q("😂")])
    ).toEqual([q("👍"), q("🙏"), q("😂")]);
  });

  it("空いていた枠には末尾に足す", () => {
    expect(mergeQuickReactionsStable([q("👍")], [q("🎉"), q("👍")])).toEqual([q("👍"), q("🎉")]);
  });
});
