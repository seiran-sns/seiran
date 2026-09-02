import { describe, expect, it } from "vitest";
import { resolveInsertion } from "./ComposerEditor";

describe("resolveInsertion", () => {
  it("通常のカーソル位置にそのまま挿入する", () => {
    expect(resolveInsertion("hello world", 5, ":smile:")).toEqual({
      next: "hello:smile: world",
      caret: 12,
    });
  });

  it("挿入直後が半角英数字なら半角スペースを1つ追加する", () => {
    expect(resolveInsertion("ab", 1, ":smile:")).toEqual({
      next: "a:smile: b",
      caret: 9,
    });
  });

  it("挿入直後が半角英数字でなければスペースを追加しない", () => {
    expect(resolveInsertion("a,b", 1, ":smile:")).toEqual({
      next: "a:smile:,b",
      caret: 8,
    });
  });

  it("カーソルが既存ショートコードの内側にある場合、その直後に挿入する", () => {
    const value = ":smile:";
    const caret = 4; // ":smi|le:"
    expect(resolveInsertion(value, caret, ":wink:")).toEqual({
      next: ":smile::wink:",
      caret: 13,
    });
  });

  it("カーソルが既存メンションの内側にある場合、その直後に挿入する", () => {
    const value = "@alice hi";
    const caret = 3; // "@al|ice hi"
    expect(resolveInsertion(value, caret, "@bob")).toEqual({
      next: "@alice@bob hi",
      caret: 10,
    });
  });

  it("カーソルがショートコードの直後（境界）にある場合も通常挿入と同じ結果になる", () => {
    const value = ":smile:";
    const caret = 7; // ":smile:|"（内側ではなく境界なのでenclosing判定には掛からない）
    expect(resolveInsertion(value, caret, ":wink:")).toEqual({
      next: ":smile::wink:",
      caret: 13,
    });
  });

  it("直後に英数字が続くショートコード形状はデコレーション対象外のため、素のカーソル位置へ挿入しスペースを補う", () => {
    // ":smile:x" は SHORTCODE_SOURCE の右端境界規則により丸ごと非デコレーション扱いとなる
    // （DECORATION_RE の否定先読みが不成立）。そのためenclosing判定には掛からず、
    // caretそのままの位置に通常挿入となり、直後が英数字ならスペース補完が働く。
    const value = ":smile:x";
    const caret = 3; // ":sm|ile:x"
    expect(resolveInsertion(value, caret, ":wink:")).toEqual({
      next: ":sm:wink: ile:x",
      caret: 10,
    });
  });
});
