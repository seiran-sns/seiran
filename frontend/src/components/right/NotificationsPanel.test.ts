import { describe, expect, it } from "vitest";
import type { NotificationItem } from "../../api/client";
import { describeNotification } from "./NotificationsPanel";

function makeReactionNotification(overrides: Partial<NotificationItem> = {}): NotificationItem {
  return {
    id: "1",
    createdAt: "2026-07-23T00:00:00Z",
    type: "reaction",
    ...overrides,
  };
}

describe("describeNotification（#61: カスタム絵文字リアクション通知の画像解決）", () => {
  // バックエンド（`convert.rs`）の `reactionEmojis` キーは Misskey 本家仕様に合わせ
  // コロンなし shortcode。`reaction` はコロン付き `:shortcode:` 形式で届くため、
  // このコロンを剥がしてから参照しないと画像が解決できず絵文字テキストにフォールバックしていた。
  it("reaction が :shortcode: 形式でも reactionEmojis のコロンなしキーで画像URLを解決できる", () => {
    const n = makeReactionNotification({
      reaction: ":blob_cat:",
      note: { id: "42", reactionEmojis: { blob_cat: "https://example.com/blob_cat.png" } },
    });
    expect(describeNotification(n).iconUrl).toBe("https://example.com/blob_cat.png");
  });

  it("reactionEmojis に対応するキーが無ければ画像URLは undefined（絵文字テキストへフォールバック）", () => {
    const n = makeReactionNotification({ reaction: ":unknown_emoji:", note: { id: "42", reactionEmojis: {} } });
    expect(describeNotification(n).iconUrl).toBeUndefined();
  });

  it("Unicode絵文字のリアクションは画像URLを持たない", () => {
    const n = makeReactionNotification({ reaction: "🎉", note: { id: "42", reactionEmojis: {} } });
    const result = describeNotification(n);
    expect(result.icon).toBe("🎉");
    expect(result.iconUrl).toBeUndefined();
  });
});
