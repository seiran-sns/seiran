import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import EmojiText from "./EmojiText";

describe("EmojiText", () => {
  it("絵文字マップにあるCW内ショートコードを画像へ展開できる", () => {
    const html = renderToStaticMarkup(
      <p>
        ⚠️ <EmojiText text="注意 :blobcat:" emojis={{ ":blobcat:": "https://example.com/blobcat.png" }} />
      </p>,
    );

    expect(html).toContain("注意 ");
    expect(html).toContain('alt=":blobcat:"');
    expect(html).toContain("blobcat.png");
  });

  it("カスタム絵文字マップが無くても本文中のUnicode絵文字はtwemoji画像へ変換する", () => {
    const html = renderToStaticMarkup(<EmojiText text="やった🎉！" />);

    expect(html).toContain("やった");
    expect(html).toContain("！");
    expect(html).toContain('alt="🎉"');
    expect(html).toContain("/twemoji/1f389.svg");
  });

  it("カスタム絵文字とUnicode絵文字が混在していても両方展開する", () => {
    const html = renderToStaticMarkup(
      <EmojiText text="わこつ:blobcat:🎉" emojis={{ ":blobcat:": "https://example.com/blobcat.png" }} />,
    );

    expect(html).toContain("わこつ");
    expect(html).toContain("blobcat.png");
    expect(html).toContain("/twemoji/1f389.svg");
  });

  it("Unicode絵文字を含まないプレーンテキストはそのまま表示する", () => {
    const html = renderToStaticMarkup(<EmojiText text="こんにちは" />);
    expect(html).toBe("こんにちは");
  });
});
