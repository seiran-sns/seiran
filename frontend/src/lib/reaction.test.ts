import { describe, expect, it } from "vitest";
import { isValidReactionEmoji } from "./reaction";

describe("isValidReactionEmoji", () => {
  it("単一の絵文字を許可する", () => {
    expect(isValidReactionEmoji("🎉")).toBe(true);
  });

  it("肌色修飾付き絵文字を許可する", () => {
    expect(isValidReactionEmoji("👍🏽")).toBe(true);
  });

  it("ZWJ結合絵文字を許可する", () => {
    expect(isValidReactionEmoji("👨‍👩‍👧‍👦")).toBe(true);
  });

  it("国旗絵文字を許可する", () => {
    expect(isValidReactionEmoji("🇯🇵")).toBe(true);
  });

  it("絵文字とテキストの混在を拒否する", () => {
    expect(isValidReactionEmoji("🎉nice")).toBe(false);
  });

  it("複数絵文字の連結を拒否する", () => {
    expect(isValidReactionEmoji("🎉🎊")).toBe(false);
  });

  it("プレーンテキストを拒否する", () => {
    expect(isValidReactionEmoji("hello")).toBe(false);
  });

  it("shortcode形式を拒否する", () => {
    expect(isValidReactionEmoji(":blobcat:")).toBe(false);
  });

  it("空文字・空白のみを拒否する", () => {
    expect(isValidReactionEmoji("")).toBe(false);
    expect(isValidReactionEmoji("   ")).toBe(false);
  });

  it("前後の空白はtrimして判定する", () => {
    expect(isValidReactionEmoji("  🎉  ")).toBe(true);
  });
});
