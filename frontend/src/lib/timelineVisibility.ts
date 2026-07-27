import type { Note } from "../api/client";
import type { Feed } from "../contexts/HomeFeedContext";

/** LTL/GTLで非公開範囲の投稿を描画しないためのクライアント側最終防御。 */
export function filterTimelineNotes(feed: Feed, notes: Note[]): Note[] {
  if (feed.kind !== "local" && feed.kind !== "global") return notes;
  return notes.filter((note) => note.visibility !== "unlisted" && note.visibility !== "followers_only");
}
