import { describe, expect, it } from "vitest";
import { loadEmojiAnnotationIndex } from "./emojiAnnotations";

describe("loadEmojiAnnotationIndex", () => {
  it("UI言語が英語なら英語版アノテーションのみを含む", async () => {
    const index = await loadEmojiAnnotationIndex("en");
    const words = index.get("🍮") ?? [];
    expect(words.some((w) => w.toLowerCase().includes("custard"))).toBe(true);
    expect(words.some((w) => w.includes("プリン"))).toBe(false);
  });

  it("UI言語が日本語なら英語版と日本語版の両方を含む（他言語は含まない）", async () => {
    const index = await loadEmojiAnnotationIndex("ja");
    const words = index.get("🍮") ?? [];
    expect(words.some((w) => w.toLowerCase().includes("custard"))).toBe(true);
    expect(words.some((w) => w.includes("プリン"))).toBe(true);
    // 韓国語アノテーション「커스타드 푸딩」はヒット対象に含まれない
    expect(words.some((w) => w.includes("커스타드"))).toBe(false);
  });
});
