import { api, PublicEmoji } from "../api/client";

let cache: Promise<PublicEmoji[]> | null = null;

/**
 * このサーバーに登録済みのカスタム絵文字一覧を返す（`GET /api/emojis`）。
 * 絵文字ピッカーと `ReactionChips` の両方が同じデータを必要とするため、プロセス内で
 * 1回だけフェッチしてキャッシュする（ページリロードまでは新規登録分は反映されない）。
 */
export function fetchCustomEmojis(): Promise<PublicEmoji[]> {
  if (!cache) {
    cache = api.emojis.list().then((res) => res.emojis);
  }
  return cache;
}

/** このサーバーに登録済みのカスタム絵文字 shortcode 一覧を `Set` で返す。 */
export function fetchCustomEmojiShortcodes(): Promise<Set<string>> {
  return fetchCustomEmojis().then((emojis) => new Set(emojis.map((e) => e.name)));
}

/**
 * `:shortcode:` または `:shortcode@host:`（本家Misskey準拠、ローカルは `@.`）を分解する。
 * `@` が無ければ `host: null`（ホスト情報なし＝レガシーデータ）を返す。
 */
export function parseReactionContent(
  content: string
): { shortcode: string; host: string | null } | null {
  if (content.length <= 2 || !content.startsWith(":") || !content.endsWith(":")) {
    return null;
  }
  const inner = content.slice(1, -1);
  if (inner.length === 0) return null;
  const atIndex = inner.indexOf("@");
  if (atIndex === -1) {
    return { shortcode: inner, host: null };
  }
  const shortcode = inner.slice(0, atIndex);
  const host = inner.slice(atIndex + 1);
  if (shortcode.length === 0 || host.length === 0) return null;
  return { shortcode, host };
}

/** ローカル絵文字判定。ホスト情報なし（レガシーデータ）または `.` はローカル相当として扱う。 */
export function isLocalCustomEmoji(parsed: { host: string | null }): boolean {
  return parsed.host === null || parsed.host === ".";
}

/** `:shortcode:` / `:shortcode@host:` 形式なら shortcode 部分のみを、そうでなければ null を返す。 */
export function parseCustomEmojiShortcode(content: string): string | null {
  return parseReactionContent(content)?.shortcode ?? null;
}
