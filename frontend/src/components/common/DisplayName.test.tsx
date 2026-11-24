import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import DisplayName from "./DisplayName";

describe("DisplayName", () => {
  it("表示名中のショートコードを画像に展開する", () => {
    const html = renderToStaticMarkup(
      <DisplayName name="ゆば:blobcat:" fallback="yuba" emojis={{ ":blobcat:": "https://example.com/blobcat.png" }} />,
    );
    expect(html).toContain("ゆば");
    expect(html).toContain("blobcat.png");
  });

  it("表示名が空ならフォールバックを出す", () => {
    expect(renderToStaticMarkup(<DisplayName name="" fallback="yuba" />)).toBe("yuba");
    expect(renderToStaticMarkup(<DisplayName name={null} fallback="yuba" />)).toBe("yuba");
  });
});
