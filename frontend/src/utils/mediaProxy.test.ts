import { beforeEach, describe, expect, it } from "vitest";
import { configureMediaProxy, mediaUrl } from "./mediaProxy";

describe("mediaUrl", () => {
  beforeEach(() => configureMediaProxy(""));

  it("proxies remote URLs through the built-in endpoint", () => {
    expect(mediaUrl("https://remote.example/emoji.png")).toBe(
      "/proxy?url=https%3A%2F%2Fremote.example%2Femoji.png",
    );
  });

  it("leaves same-origin and non-http URLs unchanged", () => {
    expect(mediaUrl(`${window.location.origin}/local.png`)).toBe(
      `${window.location.origin}/local.png`,
    );
    expect(mediaUrl("data:image/png;base64,abc")).toBe(
      "data:image/png;base64,abc",
    );
  });

  it("uses a configured Misskey-compatible external proxy", () => {
    configureMediaProxy("https://proxy.example/");
    expect(mediaUrl("https://remote.example/a.png")).toBe(
      "https://proxy.example/proxy?url=https%3A%2F%2Fremote.example%2Fa.png",
    );
  });
});
