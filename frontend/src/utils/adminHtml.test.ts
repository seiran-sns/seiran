import { describe, expect, it } from "vitest";
import { trimBlockBoundaryNewlines } from "./adminHtml";

describe("trimBlockBoundaryNewlines", () => {
  it("removes one newline between block tags", () => {
    expect(trimBlockBoundaryNewlines("<h2>A</h2>\n<h3>B</h3>\n本文")).toBe("<h2>A</h2><h3>B</h3>本文");
  });

  it("keeps extra newlines as blank lines", () => {
    expect(trimBlockBoundaryNewlines("<h2>A</h2>\n\n<h3>B</h3>")).toBe("<h2>A</h2>\n<h3>B</h3>");
    expect(trimBlockBoundaryNewlines("前文\n\n<h2>A</h2>")).toBe("前文\n<h2>A</h2>");
  });

  it("trims the start and end inside a block", () => {
    expect(trimBlockBoundaryNewlines("<p>\n一行目\n二行目\n</p>")).toBe("<p>一行目\n二行目</p>");
  });

  it("keeps newlines in text and around inline tags", () => {
    expect(trimBlockBoundaryNewlines("一行目\n二行目\n<b>太字</b>\n三行目")).toBe(
      "一行目\n二行目\n<b>太字</b>\n三行目",
    );
  });

  it("removes indentation-only gaps between block tags", () => {
    expect(trimBlockBoundaryNewlines("<ul>\n  <li>A</li>\n  <li>B</li>\n</ul>")).toBe(
      "<ul><li>A</li><li>B</li></ul>",
    );
  });

  it("treats style as a block and ignores comments", () => {
    expect(trimBlockBoundaryNewlines("<style>\nh3 {}\n</style>\n<!-- c -->\n<p>x</p>")).toBe(
      "<style>h3 {}</style><!-- c --><p>x</p>",
    );
  });

  it("leaves plain text untouched", () => {
    expect(trimBlockBoundaryNewlines("第1条\n\n本規約は…\n")).toBe("第1条\n\n本規約は…\n");
  });
});
