import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import RichText from "./RichText";

function render(text: string, emojis?: Record<string, string>) {
  return renderToStaticMarkup(
    <MemoryRouter initialEntries={["/"]}>
      <RichText text={text} emojis={emojis} />
    </MemoryRouter>,
  );
}

describe("RichText", () => {
  it("本文中のUnicode絵文字をtwemoji画像へ変換する", () => {
    const html = render("やった🎉！");
    expect(html).toContain("やった");
    expect(html).toContain("！");
    expect(html).toContain('alt="🎉"');
    expect(html).toContain("/twemoji/1f389.svg");
  });

  it("リンク・メンション・カスタム絵文字ショートコードと絵文字混在でも両方展開する", () => {
    const html = render("わこつ🎉 @alice :blobcat:", { ":blobcat:": "https://example.com/blobcat.png" });
    expect(html).toContain("/twemoji/1f389.svg");
    expect(html).toContain("blobcat.png");
    expect(html).toContain("@alice");
  });

  it("絵文字を含まない本文はそのまま表示する", () => {
    const html = render("こんにちは");
    expect(html).toBe("こんにちは");
  });
});
