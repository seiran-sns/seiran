import { describe, expect, it } from "vitest";
import { HASHTAG_LINK_TEXT_RE, RICH_TEXT_SOURCE } from "./richTextPatterns";

function matchAll(text: string): string[] {
  const re = new RegExp(RICH_TEXT_SOURCE, "gu");
  return [...text.matchAll(re)].map((m) => m[0]);
}

describe("RICH_TEXT_SOURCE — Markdownリンク", () => {
  it("外部リンク(https)を検出する", () => {
    expect(matchAll("[seiran](https://example.com/)")).toEqual(["[seiran](https://example.com/)"]);
  });

  it("内部リンク(/始まり)を検出する", () => {
    expect(matchAll("[home](/notes/123)")).toEqual(["[home](/notes/123)"]);
  });

  it("プロトコル相対URL(//)は内部リンクとして扱わない（外部URL側にフォールバック）", () => {
    // linkUrl 側は `/(?!/)` で `//` を拒否するため、Markdownリンクとしてはマッチしない。
    const re = new RegExp(RICH_TEXT_SOURCE, "gu");
    const matches = [...("[x](//evil.example.com)".matchAll(re))];
    expect(matches.some((m) => m.groups?.linkUrl === "//evil.example.com")).toBe(false);
  });
});

describe("RICH_TEXT_SOURCE — 生URL", () => {
  it("本文中の生URLを検出する", () => {
    expect(matchAll("見て https://example.com/path こちら")).toEqual(["https://example.com/path"]);
  });
});

describe("RICH_TEXT_SOURCE — メンション", () => {
  it("@user@host形式を検出する", () => {
    expect(matchAll("こんにちは @alice@mastodon.social です")).toEqual(["@alice@mastodon.social"]);
  });

  it("ローカル@userのみも検出する", () => {
    expect(matchAll("@alice さん")).toEqual(["@alice"]);
  });

  it("メールアドレスの@はメンションとして検出しない", () => {
    expect(matchAll("admin@example.com 宛に連絡")).toEqual([]);
  });
});

describe("RICH_TEXT_SOURCE — ハッシュタグ", () => {
  it("#タグを検出する", () => {
    expect(matchAll("今日は #seiran のリリース日")).toEqual(["#seiran"]);
  });

  it("URLフラグメント(page#section)は誤検出しない", () => {
    expect(matchAll("https://example.com/page#section")).toEqual(["https://example.com/page#section"]);
  });

  it("純数字のみのハッシュタグ(#2026)は検出しない", () => {
    expect(matchAll("#2026 年")).toEqual([]);
  });

  it("英数字混在のハッシュタグは検出する", () => {
    expect(matchAll("#seiran2026")).toEqual(["#seiran2026"]);
  });
});

describe("RICH_TEXT_SOURCE — 絵文字ショートコード", () => {
  it(":shortcode: を検出する", () => {
    expect(matchAll("やった :blobcat: !")).toEqual([":blobcat:"]);
  });
});

describe("HASHTAG_LINK_TEXT_RE", () => {
  it("#タグ形状のリンクテキストにマッチする", () => {
    expect(HASHTAG_LINK_TEXT_RE.test("#seiran")).toBe(true);
  });

  it("タグ以外のテキストにはマッチしない", () => {
    expect(HASHTAG_LINK_TEXT_RE.test("home")).toBe(false);
  });

  it("純数字のみは#タグ形状として扱わない", () => {
    expect(HASHTAG_LINK_TEXT_RE.test("#2026")).toBe(false);
  });
});
