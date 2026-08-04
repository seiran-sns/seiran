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
});
