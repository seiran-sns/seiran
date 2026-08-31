import { describe, expect, it } from "vitest";
import type { Note } from "../api/client";
import {
  acct,
  calcRemaining,
  countGraphemes,
  countUtf8Bytes,
  deliveryBadges,
  displayName,
  formatCount,
  pollRemainingTime,
  profilePath,
  profileQuery,
  protocolBadge,
  visibilityBadge,
} from "./format";

function makeNote(overrides: Partial<Note> = {}): Note {
  return {
    id: "1",
    text: "hello",
    createdAt: "2026-07-21T00:00:00Z",
    user: {
      id: 1,
      username: "alice",
      actorType: "local",
    },
    attachments: [],
    replyFediAllowed: true,
    replyBskyAllowed: true,
    replyBlocked: false,
    quoteBlocked: false,
    replyCount: 0,
    quoteCount: 0,
    repostCount: 0,
    linkCards: [],
    ...overrides,
  };
}

describe("formatCount", () => {
  it("1000未満はそのまま表示する", () => {
    expect(formatCount(0)).toBe("0");
    expect(formatCount(999)).toBe("999");
  });

  it("1000以上はKで小数点以下1桁まで表示する", () => {
    expect(formatCount(1000)).toBe("1.0K");
    expect(formatCount(1234)).toBe("1.2K");
    expect(formatCount(9960)).toBe("9.9K");
  });

  it("1万以上のKは小数点なしで表示する", () => {
    expect(formatCount(12345)).toBe("12K");
    expect(formatCount(999499)).toBe("999K");
  });

  it("100万以降はMで表示する", () => {
    expect(formatCount(1_000_000)).toBe("1.0M");
    expect(formatCount(2_500_000)).toBe("2.5M");
    expect(formatCount(12_000_000)).toBe("12M");
  });
});

describe("displayName", () => {
  it("displayName があればそれを使う", () => {
    expect(displayName(makeNote({ user: { id: 1, username: "alice", displayName: "Alice", actorType: "local" } }))).toBe("Alice");
  });

  it("displayName が無ければ username にフォールバックする", () => {
    expect(displayName(makeNote({ user: { id: 1, username: "alice", actorType: "local" } }))).toBe("alice");
  });
});

describe("acct", () => {
  it("domain があれば @user@domain 形式になる", () => {
    expect(acct(makeNote({ user: { id: 1, username: "alice", domain: "example.com", actorType: "fedi" } }))).toBe(
      "@alice@example.com"
    );
  });

  it("domain が無ければ @user 形式になる", () => {
    expect(acct(makeNote({ user: { id: 1, username: "alice", actorType: "local" } }))).toBe("@alice");
  });
});

describe("profileQuery / profilePath", () => {
  it("ローカルドメイン（window.location.hostname）と一致する場合は domain を省略する", () => {
    expect(profileQuery("alice", window.location.hostname)).toBe("alice");
    expect(profilePath("alice", window.location.hostname)).toBe("/@alice");
  });

  it("リモートドメインは付与する", () => {
    expect(profileQuery("alice", "mastodon.social")).toBe("alice@mastodon.social");
    expect(profilePath("alice", "mastodon.social")).toBe("/@alice@mastodon.social");
  });

  it("domain 未指定はそのまま username", () => {
    expect(profileQuery("alice")).toBe("alice");
  });
});

describe("protocolBadge", () => {
  it("bsky はバッジを返す", () => {
    expect(protocolBadge("bsky")).toEqual({ icon: "🦋", label: "Bluesky" });
  });

  it("fedi はバッジを返す", () => {
    expect(protocolBadge("fedi")).toEqual({ icon: "🌐", label: "Fediverse" });
  });

  it("remote_seiran はバッジを返す", () => {
    expect(protocolBadge("remote_seiran")).toEqual({ icon: "🀄", label: "seiran" });
  });

  it("local はバッジ無し", () => {
    expect(protocolBadge("local")).toBeNull();
  });

  it("未知の actorType はバッジ無し", () => {
    expect(protocolBadge("unknown")).toBeNull();
  });
});

describe("deliveryBadges", () => {
  it("ローカル投稿以外は常に空配列", () => {
    expect(deliveryBadges(makeNote({ user: { id: 1, username: "a", actorType: "fedi" }, deliverFedi: true }))).toEqual([]);
  });

  it("Fedi配送ありのローカル投稿はfediバッジを含む", () => {
    const badges = deliveryBadges(makeNote({ deliverFedi: true }));
    expect(badges.map((b) => b.protocol)).toContain("fedi");
  });

  it("Bsky配送ありのローカル投稿はbskyバッジを含む", () => {
    const badges = deliveryBadges(makeNote({ deliverBsky: true }));
    expect(badges.map((b) => b.protocol)).toContain("bsky");
  });

  it("配送先が無ければ空配列", () => {
    expect(deliveryBadges(makeNote({ deliverFedi: false, deliverBsky: false }))).toEqual([]);
  });
});

describe("visibilityBadge", () => {
  it("followers_only は🔒️バッジ", () => {
    expect(visibilityBadge(makeNote({ visibility: "followers_only" }))?.icon).toBe("🔒️");
  });

  it("unlisted は🤫バッジ", () => {
    expect(visibilityBadge(makeNote({ visibility: "unlisted" }))?.icon).toBe("🤫");
  });

  it("public(未指定)はバッジ無し", () => {
    expect(visibilityBadge(makeNote({}))).toBeNull();
  });

  it("direct はバッジ無し", () => {
    expect(visibilityBadge(makeNote({ visibility: "direct" }))).toBeNull();
  });
});

describe("countGraphemes / countUtf8Bytes", () => {
  it("ASCII文字はgrapheme数=文字数", () => {
    expect(countGraphemes("hello")).toBe(5);
  });

  it("サロゲートペア絵文字は1graphemeとして数える", () => {
    expect(countGraphemes("🎉")).toBe(1);
    expect("🎉".length).toBe(2); // UTF-16コード単位では2（比較対象として明記）
  });

  it("ZWJ結合絵文字も1graphemeとして数える", () => {
    expect(countGraphemes("👨‍👩‍👧‍👦")).toBe(1);
  });

  it("UTF-8バイト数はASCIIで1バイト/文字", () => {
    expect(countUtf8Bytes("abc")).toBe(3);
  });

  it("日本語はUTF-8で3バイト/文字", () => {
    expect(countUtf8Bytes("あ")).toBe(3);
  });
});

describe("calcRemaining", () => {
  it("Bsky配信時は300grapheme/3000B基準", () => {
    expect(calcRemaining("", true)).toBe(300);
  });

  it("非Bsky配信時は3000grapheme/10000B基準", () => {
    expect(calcRemaining("", false)).toBe(3000); // floor(10000/3)=3333 > 3000 なのでgrapheme側が効く
  });

  it("文字数が増えるほど残数が減る", () => {
    expect(calcRemaining("a".repeat(10), true)).toBe(290);
  });

  it("上限超過時は負数になる", () => {
    expect(calcRemaining("a".repeat(301), true)).toBeLessThan(0);
  });
});

describe("pollRemainingTime", () => {
  it("残り1分未満なら秒単位（切り捨て）で返す", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    const endTime = new Date(now + 45 * 1000).toISOString();
    expect(pollRemainingTime(endTime, now)).toEqual({ tier: "seconds", seconds: 45 });
  });

  it("残り1分以上1時間未満なら分単位（切り捨て）で返す", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    const endTime = new Date(now + 90 * 1000).toISOString(); // 残り1分30秒
    expect(pollRemainingTime(endTime, now)).toEqual({ tier: "minutes", minutes: 1 });
  });

  it("ちょうど60秒は分単位になる", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    const endTime = new Date(now + 60 * 1000).toISOString();
    expect(pollRemainingTime(endTime, now)).toEqual({ tier: "minutes", minutes: 1 });
  });

  it("残り1時間以上1日未満なら「時間+分」で返す", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    const endTime = new Date(now + (90 * 60 + 5) * 1000).toISOString(); // 1時間30分5秒
    expect(pollRemainingTime(endTime, now)).toEqual({ tier: "hours", hours: 1, minutes: 30 });
  });

  it("ちょうど60分は「1時間0分」になる", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    const endTime = new Date(now + 60 * 60 * 1000).toISOString();
    expect(pollRemainingTime(endTime, now)).toEqual({ tier: "hours", hours: 1, minutes: 0 });
  });

  it("残り1日以上なら「日+時間+分」で返す", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    // 2日1時間30分
    const totalSeconds = (2 * 24 * 60 + 1 * 60 + 30) * 60;
    const endTime = new Date(now + totalSeconds * 1000).toISOString();
    expect(pollRemainingTime(endTime, now)).toEqual({
      tier: "days",
      days: 2,
      hours: 1,
      minutes: 30,
    });
  });

  it("ちょうど24時間は「1日0時間0分」になる", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    const endTime = new Date(now + 24 * 60 * 60 * 1000).toISOString();
    expect(pollRemainingTime(endTime, now)).toEqual({
      tier: "days",
      days: 1,
      hours: 0,
      minutes: 0,
    });
  });

  it("期限を過ぎていればnull", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    const endTime = new Date(now - 1000).toISOString();
    expect(pollRemainingTime(endTime, now)).toBeNull();
  });

  it("ちょうど期限（残り0）もnull", () => {
    const now = Date.parse("2026-08-24T00:00:00Z");
    expect(pollRemainingTime(new Date(now).toISOString(), now)).toBeNull();
  });

  it("不正な日時文字列ならnull", () => {
    expect(pollRemainingTime("not-a-date", Date.now())).toBeNull();
  });
});
