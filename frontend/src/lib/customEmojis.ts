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

/** `:shortcode:` 形式ならコロンを除いた shortcode を、そうでなければ null を返す。 */
export function parseCustomEmojiShortcode(content: string): string | null {
  if (content.length > 2 && content.startsWith(":") && content.endsWith(":")) {
    return content.slice(1, -1);
  }
  return null;
}
