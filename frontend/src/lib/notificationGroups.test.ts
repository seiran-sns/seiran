import { describe, expect, it } from "vitest";
import type { NotificationItem } from "../api/client";
import { groupNotifications } from "./notificationGroups";

let seq = 0;
/** `user` が `post` にリアクションした通知。 */
function r(user: string, post: string): NotificationItem {
  seq += 1;
  return {
    id: `n${seq}-${user}-${post}`,
    createdAt: "",
    type: "reaction",
    user: { id: user, username: user, host: null },
    note: { id: post },
  };
}

/** `user` が `post` をリポストした通知（`note` はリポストラッパー）。 */
function rp(user: string, post: string): NotificationItem {
  seq += 1;
  return {
    id: `n${seq}-rp-${user}-${post}`,
    createdAt: "",
    type: "renote",
    user: { id: user, username: user, host: null },
    note: { id: `w${seq}`, renote: { id: post } },
  };
}

function follow(user: string): NotificationItem {
  seq += 1;
  return { id: `n${seq}-follow-${user}`, createdAt: "", type: "follow", user: { id: user, username: user, host: null } };
}

function n(type: string, user: string, relatedUser?: string): NotificationItem {
  seq += 1;
  return {
    id: `n${seq}-${type}-${user}`,
    createdAt: "",
    type,
    user: { id: user, username: user, host: null },
    ...(relatedUser ? { relatedUser: { id: relatedUser, username: relatedUser, host: null } } : {}),
  };
}

/** まとまりを「件数」の列で表す。 */
function sizes(items: NotificationItem[], baseOldestId?: string): number[] {
  return groupNotifications(items, baseOldestId).map((g) => g.items.length);
}

describe("groupNotifications", () => {
  it("同じポストへの連続と同じ人からの連続をまとめる", () => {
    // 新しい順。
    const items = [r("a", "P1"), r("b", "P1"), r("c", "P1"), r("c", "P2"), r("c", "P3")];
    // 古い側から: c-P3, c-P2, c-P1 が同じ人でまとまり、残りの b-P1, a-P1 が同じポスト。
    expect(sizes(items)).toEqual([2, 3]);
  });

  it("系統の違う通知とはまとめず、連続を切る", () => {
    const items = [r("a", "P1"), follow("x"), r("a", "P1")];
    expect(sizes(items)).toEqual([1, 1, 1]);
    // 同じ人でも反応と関係はまとめない。
    expect(sizes([follow("a"), r("a", "P1")])).toEqual([1, 1]);
  });

  it("フォロー・フォロー承認・引っ越しは同じ人ならまとめる", () => {
    const items = [n("followRequestAccepted", "a"), follow("a"), follow("b"), follow("c")];
    expect(sizes(items)).toEqual([2, 1, 1]);
  });

  it("引っ越し通知は引っ越し先の人でまとめる", () => {
    // old から new へ引っ越した通知と、new からのフォロー。
    expect(sizes([follow("new"), n("moveRefollowed", "old", "new")])).toEqual([2]);
    expect(sizes([follow("old"), n("moveAlreadyFollowing", "old", "new")])).toEqual([1, 1]);
  });

  it("メンション・返信・引用・フォローリクエストはまとめない", () => {
    expect(sizes([n("reply", "a"), n("reply", "a")])).toEqual([1, 1]);
    expect(sizes([n("mention", "a"), n("quote", "a")])).toEqual([1, 1]);
    expect(sizes([n("receiveFollowRequest", "a"), follow("a")])).toEqual([1, 1]);
  });

  it("リポスト通知もリポスト元のポストと人でリアクション通知とまとめる", () => {
    // 古い側から: c-P2 のリポスト・c-P1 のリアクションが同じ人、残りが P1 で同じポスト。
    const items = [rp("a", "P1"), r("b", "P1"), r("c", "P1"), rp("c", "P2")];
    expect(sizes(items)).toEqual([2, 2]);
  });

  it("同じ人から同じポストへの通知が続く間はどちらの規則でまとめるかを保留する", () => {
    // 古い側から a-P1 のリポスト・a-P1 のリアクションの時点では規則が決まらない。
    // 続く b-P1 で「同じポスト」に決まり、3件で1つになる。
    expect(sizes([r("b", "P1"), r("a", "P1"), rp("a", "P1")])).toEqual([3]);
    // 続く a-P2 なら「同じ人」に決まる。
    expect(sizes([r("a", "P2"), r("a", "P1"), rp("a", "P1")])).toEqual([3]);
  });

  it("取り消し済みでラッパーが残っていないリポスト通知は note 自体をリポスト元とみなす", () => {
    const withdrawn: NotificationItem = { ...rp("a", "P1"), repostWithdrawn: true, note: { id: "P1" } };
    expect(sizes([r("b", "P1"), withdrawn])).toEqual([2]);
  });

  it("まとまりの数が最小になる", () => {
    // a-P1 | a-P2 b-P2 | b-P3 のどこで切るかで3にも4にもなりうる並び。
    const items = [r("a", "P1"), r("a", "P2"), r("b", "P2"), r("b", "P3")];
    expect(sizes(items)).toHaveLength(2);
  });

  it("上に新着が足されても、動くのは一番上のまとまりだけ", () => {
    const base = [r("a", "P1"), r("b", "P2"), r("b", "P3"), r("c", "P3")];
    const before = groupNotifications(base).map((g) => g.items.map((n) => n.id));
    const withNew = [r("x", "P9"), r("a", "P1"), ...base];
    const after = groupNotifications(withNew).map((g) => g.items.map((n) => n.id));
    // 一番上以外は同じ。
    expect(after.slice(-(before.length - 1))).toEqual(before.slice(1));
  });

  it("下に古い通知を読み込んでも、表示済みのまとまりの区切りは変わらない", () => {
    const first = [r("a", "P1"), r("b", "P1"), r("b", "P2")];
    const baseOldestId = first[first.length - 1].id;
    const before = groupNotifications(first, baseOldestId).map((g) => g.items.map((n) => n.id));
    // 古い側で c-P2 が続くと、全体を組み直せば b-P2 は c-P2 と同じポストでまとまりうる。
    const older = [r("c", "P2"), r("d", "P2")];
    const after = groupNotifications([...first, ...older], baseOldestId).map((g) => g.items.map((n) => n.id));
    for (const group of before) {
      const found = after.find((g) => g.includes(group[0]));
      expect(found?.slice(0, group.length)).toEqual(group);
    }
  });

  it("境の通知が一覧から消えたら全体を組み直す", () => {
    const items = [r("a", "P1"), r("a", "P2")];
    expect(sizes(items, "missing")).toEqual([2]);
  });
});
