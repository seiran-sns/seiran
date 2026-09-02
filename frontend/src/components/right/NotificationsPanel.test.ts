import { describe, expect, it } from "vitest";
import type { NotificationItem } from "../../api/client";
import { describeNotification, resolveClickTargetNoteId, resolveTargetNoteId } from "./NotificationsPanel";

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

describe("describeNotification（AP Move受信時の独自拡張通知）", () => {
  it("moveRefollowed は移転先の relatedUser からハンドルを組み立てる", () => {
    const n = makeReactionNotification({
      type: "moveRefollowed",
      user: { id: "1", username: "alice", host: "old.example" },
      relatedUser: { id: "2", username: "alice", host: "new.example" },
    });
    const result = describeNotification(n);
    expect(result.i18nKey).toBe("notifications:notificationsPanel.moveRefollowedText");
    expect(result.newHandle).toBe("@alice@new.example");
  });

  it("moveAlreadyFollowing も同様に relatedUser のハンドルを組み立てる", () => {
    const n = makeReactionNotification({
      type: "moveAlreadyFollowing",
      user: { id: "1", username: "alice", host: "old.example" },
      relatedUser: { id: "2", username: "alice", host: "new.example" },
    });
    expect(describeNotification(n).i18nKey).toBe(
      "notifications:notificationsPanel.moveAlreadyFollowingText"
    );
  });

  it("relatedUser の host が無ければ name/username にフォールバックする", () => {
    const n = makeReactionNotification({
      type: "moveRefollowed",
      relatedUser: { id: "2", username: "alice", host: null, name: "Alice" },
    });
    expect(describeNotification(n).newHandle).toBe("Alice");
  });
});

describe("resolveTargetNoteId（リポスト通知のダイジェスト対象は元投稿にする）", () => {
  it("renote通知（Misskey本家仕様のリポスト種別名）はリポストラッパー自身ではなく note.renote.id を返す", () => {
    const n = makeReactionNotification({
      type: "renote",
      note: { id: "999", renote: { id: "42" } },
    });
    expect(resolveTargetNoteId(n)).toBe("42");
  });

  it("renote通知でも renote が埋め込まれていなければラッパー自身の id にフォールバックする", () => {
    const n = makeReactionNotification({ type: "renote", note: { id: "999" } });
    expect(resolveTargetNoteId(n)).toBe("999");
  });

  it("quote通知は引用投稿自体（note.id）を対象にする（renoteは見ない）", () => {
    const n = makeReactionNotification({
      type: "quote",
      note: { id: "999", renote: { id: "42" } },
    });
    expect(resolveTargetNoteId(n)).toBe("999");
  });

  it("followのようにポストへのリンクを持たない通知種別は undefined を返す", () => {
    const n = makeReactionNotification({ type: "follow", note: { id: "999" } });
    expect(resolveTargetNoteId(n)).toBeUndefined();
  });
});

describe("resolveClickTargetNoteId（クリック遷移先はリポストラッパー投稿自身にする）", () => {
  it("renote通知はリポスト元ではなくラッパー投稿自身（note.id）を返す", () => {
    const n = makeReactionNotification({
      type: "renote",
      note: { id: "999", renote: { id: "42" } },
    });
    expect(resolveClickTargetNoteId(n)).toBe("999");
  });

  it("quote通知は引用投稿自体（note.id）を対象にする", () => {
    const n = makeReactionNotification({
      type: "quote",
      note: { id: "999", renote: { id: "42" } },
    });
    expect(resolveClickTargetNoteId(n)).toBe("999");
  });

  it("followのようにポストへのリンクを持たない通知種別は undefined を返す", () => {
    const n = makeReactionNotification({ type: "follow", note: { id: "999" } });
    expect(resolveClickTargetNoteId(n)).toBeUndefined();
  });
});
