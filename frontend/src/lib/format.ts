import { Note } from "../api/client";

/** ISO 文字列を日本語ロケールの短い日時表記に変換する。 */
export function formatDate(iso: string): string {
  return new Date(iso).toLocaleString("ja-JP", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** ノートの表示名（display_name 優先、なければ username）。 */
export function displayName(note: Note): string {
  return note.user.displayName || note.user.username;
}

/** ノート投稿者の acct 文字列（`@user` または `@user@domain`）。 */
export function acct(note: Note): string {
  return note.user.domain
    ? `@${note.user.username}@${note.user.domain}`
    : `@${note.user.username}`;
}

/** プロフィール遷移用のクエリ文字列を組み立てる（ローカルは domain を省略）。 */
export function profileQuery(username: string, domain?: string): string {
  return domain && domain !== window.location.hostname
    ? `${username}@${domain}`
    : username;
}

/** プロフィールの permalink パス（Misskey 互換の `/@handle` 形式・#36）。 */
export function profilePath(username: string, domain?: string): string {
  return `/@${profileQuery(username, domain)}`;
}

/** アクター種別に対応するプロトコルバッジ（絵文字 + ラベル）。 */
export function protocolBadge(actorType: string): { icon: string; label: string } | null {
  switch (actorType) {
    case "bsky":
      return { icon: "🦋", label: "Bluesky" };
    case "fedi":
      return { icon: "🌐", label: "Fediverse" };
    case "remote_seiran":
      return { icon: "🀄", label: "seiran" };
    case "local":
      return null; // ローカルはバッジ不要
    default:
      return null;
  }
}

const segmenter = new Intl.Segmenter();

export function countGraphemes(text: string): number {
  return [...segmenter.segment(text)].length;
}

export function countUtf8Bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** Bsky 配信時は 300grapheme/3,000B、それ以外は 3,000grapheme/10,000B の残数を返す。 */
export function calcRemaining(text: string, deliverBsky: boolean): number {
  const maxBytes = deliverBsky ? 3_000 : 10_000;
  const maxGraphemes = deliverBsky ? 300 : 3_000;
  const graphemes = countGraphemes(text);
  const bytes = countUtf8Bytes(text);
  return Math.min(maxGraphemes - graphemes, Math.floor((maxBytes - bytes) / 3));
}
