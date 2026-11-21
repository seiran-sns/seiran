import type { Note } from "../api/client";

/**
 * タイムラインで重ねて表示するポストのまとまり。`notes` は一覧と同じ順（新しい順）。
 *
 * 同じポストのリポストが連続していたら1つのまとまりにする。リポスト元のポスト自体が隣に
 * 並んでいればそれも含める。それ以外のポストは1件で1つ。
 */
export interface TimelineGroup {
  notes: Note[];
}

/** リポストならリポスト元、そうでなければ自分自身のID。 */
function targetId(note: Note): string {
  return note.renote?.id ?? note.renoteId ?? note.id;
}

function isRepost(note: Note): boolean {
  return note.renote !== undefined || note.renoteId !== undefined;
}

/**
 * ポストを重ねるまとまりに分ける。`breakBeforeIds` のポストの手前では切る（取りこぼし区間の
 * 区切りをまたいでまとめない）。
 *
 * 「同じポストを指す」は1つのキーの一致なので、連続する同じキーの区間がそのまままとまりに
 * なる。上に新着が入っても下に古いポストを読み込んでも、動くのは端のまとまりだけ。
 */
export function groupTimeline(notes: Note[], breakBeforeIds?: Set<string>): TimelineGroup[] {
  const runs: Note[][] = [];
  for (const note of notes) {
    const last = runs[runs.length - 1];
    if (last && !breakBeforeIds?.has(note.id) && targetId(last[0]) === targetId(note)) last.push(note);
    else runs.push([note]);
  }
  // リポストを含まない区間（同じポストが続くことは無いので実質1件）はまとめない。
  return runs.flatMap((run) => (run.some(isRepost) ? [{ notes: run }] : run.map((n) => ({ notes: [n] }))));
}
