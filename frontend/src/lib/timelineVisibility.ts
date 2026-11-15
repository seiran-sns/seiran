import type { Note } from "../api/client";
import type { Feed } from "../contexts/HomeFeedContext";

/** LTL/GTLで非公開範囲の投稿を描画しないためのクライアント側最終防御。管理者の
 * モデレーション表示のLTLは、ひかえめ・プライベートを見せること自体が目的なので除外しない。 */
export function filterTimelineNotes(feed: Feed, notes: Note[]): Note[] {
  if (feed.kind !== "local" && feed.kind !== "global") return notes;
  if (feed.kind === "local" && feed.moderation) return notes;
  return notes.filter((note) => note.visibility !== "unlisted" && note.visibility !== "followers_only");
}
