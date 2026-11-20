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

function follow(user: string): NotificationItem {
  seq += 1;
  return { id: `n${seq}-follow-${user}`, createdAt: "", type: "follow", user: { id: user, username: user, host: null } };
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

  it("リアクション以外の通知はまとめず、連続を切る", () => {
    const items = [r("a", "P1"), follow("x"), r("a", "P1")];
    expect(sizes(items)).toEqual([1, 1, 1]);
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
