import { api, ReactionActor, ReactionSummary } from "../api/client";

/**
 * リポストラッパー越しに元ポストを表示するとき、元ポストのリアクションにラッパー自身へ付いた
 * リアクションを合算した表示用の配列を返す。seiranではBskyポストをFedi方面へリポストすると
 * Fedi側からはラッパーが通常のノートに見え、リアクション（Like）がラッパー宛てに届くため
 * （逆方向も同様）、合算しないとそれらが表示される場所が無くなる。
 *
 * `reactedByMe` は元ポスト側の値だけを使う。リアクションの送信・取消は常に元ポストへ行うので、
 * ラッパー側の自分のリアクションを反映するとチップのトグル方向と実際の操作が食い違う。
 * Misskey互換APIの同じ扱いは `handlers::misskey::convert::merge_wrapper_reactions`。
 */
export function mergeRepostReactions(
  original: ReactionSummary[],
  wrapper: ReactionSummary[]
): ReactionSummary[] {
  if (wrapper.length === 0) return original;
  const merged = original.map((r) => ({ ...r }));
  for (const w of wrapper) {
    const existing = merged.find((r) => r.emoji === w.emoji);
    if (existing) {
      existing.count += w.count;
      existing.emojiUrl ??= w.emojiUrl;
    } else {
      merged.push({ emoji: w.emoji, count: w.count, emojiUrl: w.emojiUrl, reactedByMe: false });
    }
  }
  return merged.sort((a, b) => b.count - a.count);
}

/** 指定絵文字を付けたアクター一覧を、元ポストと（あれば）リポストラッパーの両方から取得して
 * 重複を除いて返す（`mergeRepostReactions` で合算した件数と一覧を一致させるため）。 */
export async function fetchReactionActors(
  noteId: string,
  viaRepostId: string | undefined,
  emoji: string
): Promise<ReactionActor[]> {
  const ids = viaRepostId ? [noteId, viaRepostId] : [noteId];
  const results = await Promise.all(ids.map((id) => api.notes.reactionActors(id, emoji)));
  const byId = new Map<string, ReactionActor>();
  for (const a of results.flatMap((res) => res.actors)) {
    if (!byId.has(a.id)) byId.set(a.id, a);
  }
  return [...byId.values()];
}
