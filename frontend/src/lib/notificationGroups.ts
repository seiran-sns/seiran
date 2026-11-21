import type { NotificationItem } from "../api/client";

/**
 * 通知欄で重ねて表示する通知のまとまり。`items` は新しい順（一覧と同じ順）。
 *
 * まとめるのはリアクション通知とリポスト通知で、「同じポストへの連続」か「同じ人からの連続」の
 * どちらかを満たす区間を1つのまとまりにする（種別が混ざってもよい）。それ以外の通知は1件で1つ。
 */
export interface NotificationGroup {
  items: NotificationItem[];
}

const GROUPED_TYPES = new Set(["reaction", "renote"]);

/** 反応された自分のポスト。リポスト通知の `note` はリポストラッパーなので、リポスト元を見る。 */
function postKey(n: NotificationItem): string | undefined {
  if (n.type === "reaction") return n.note?.id;
  if (n.type === "renote") return n.note?.renote?.id ?? n.note?.id;
  return undefined;
}

function userKey(n: NotificationItem): string | undefined {
  return GROUPED_TYPES.has(n.type) ? (n.user?.id ?? n.userId) : undefined;
}

/**
 * `group` に `n` を足しても「全部同じポスト」か「全部同じ人」のままか。
 *
 * まとまり全体で判定するので、同じ人から同じポストへの通知だけが並んでいる間は、どちらの規則で
 * まとめているかを決めずに保留し、両方の続きを受け付ける（最初の2件で規則を決めると、
 * 3件目が他の人から同じポストへの通知のときにまとめ損ねる）。
 */
function canJoin(group: NotificationItem[], n: NotificationItem): boolean {
  const post = postKey(n);
  const user = userKey(n);
  if (post === undefined && user === undefined) return false;
  const samePost = post !== undefined && group.every((g) => postKey(g) === post);
  const sameUser = user !== undefined && group.every((g) => userKey(g) === user);
  return samePost || sameUser;
}

/** `ordered` を先頭から順に、まとめられる限り前のまとまりへ足していく。 */
function greedy(ordered: NotificationItem[], start: NotificationItem[][] = []): NotificationItem[][] {
  const groups = start;
  for (const n of ordered) {
    const last = groups[groups.length - 1];
    if (last && canJoin(last, n)) last.push(n);
    else groups.push([n]);
  }
  return groups;
}

/**
 * 通知を重ねるまとまりに分ける。
 *
 * 「全部同じポスト」「全部同じ人」は区間の一部を切り出しても満たされるので、端から
 * まとめられる限り長く取る貪欲法でまとまりの数が最小になる。
 *
 * 通知は上（新着）にも下（古い通知の追加読み込み）にも増える。表示中のまとまりが組み
 * 替わらないよう、`baseOldestId`（最初にまとめたときの一番古い通知）を境に分けて扱う:
 * - それより新しい側は古い側から貪欲にまとめる。新着が来ても動くのは一番上のまとまりだけ。
 * - それより古い側（後から読み込んだ分）は、境のまとまりの続きとして新しい側から貪欲に
 *   まとめる。既に表示しているまとまりの区切りは変わらない。
 * `baseOldestId` が一覧に無ければ（一覧を取り直した等）全体を古い側からまとめる。
 */
export function groupNotifications(items: NotificationItem[], baseOldestId?: string): NotificationGroup[] {
  const baseIndex = baseOldestId === undefined ? -1 : items.findIndex((n) => n.id === baseOldestId);
  const upper = baseIndex === -1 ? items : items.slice(0, baseIndex + 1);
  const lower = baseIndex === -1 ? [] : items.slice(baseIndex + 1);

  // 新しい側: 古い順に並べて貪欲にまとめ、新しい順へ戻す。
  const upperGroups = greedy([...upper].reverse())
    .map((g) => g.reverse())
    .reverse();
  // 古い側: 境のまとまり（upperGroups の最後）の続きとして新しい順にまとめる。
  const groups = greedy(lower, upperGroups);
  return groups.map((g) => ({ items: g }));
}
