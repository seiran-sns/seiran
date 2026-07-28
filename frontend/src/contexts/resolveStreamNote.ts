import { api, type Note, noteFromStream } from "../api/client";

/**
 * WebSocket の note は受信経路によって簡易ペイロードの場合があるため、DB保存後の
 * 権威的な NoteResponse で補完する。取得に失敗してもリアルタイム表示は維持する。
 */
export async function resolveStreamNote(
  body: unknown,
  fetchNote: (id: string) => Promise<Note> = api.notes.get,
): Promise<Note> {
  const streamed = noteFromStream(body);
  try {
    return await fetchNote(streamed.id);
  } catch {
    return streamed;
  }
}
