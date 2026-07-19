/**
 * 本文中のリッチテキスト要素（Markdownリンク・生URL・メンション・絵文字ショートコード）を
 * 検出するための正規表現ソース文字列。`EmojiText`/`RichText` の両方から使われる。
 */

/** 絵文字ショートコード（`:name:`）。 */
export const SHORTCODE_SOURCE = ":[a-zA-Z0-9_+-]+:";

/** 英数字・アンダースコア（ショートコードの構成文字と同じ集合）。右端の接触判定に使う。 */
export const WORD_CHAR_RE = /[a-zA-Z0-9_]/;

/**
 * Markdownリンク `[text](url)`。内部リンクは `/` 始まり（`//`＝プロトコル相対URLは除外し
 * 外部リンク扱いにする）、外部リンクは `http(s)://` 始まりのみを許可する。
 */
const MARKDOWN_LINK_SOURCE = String.raw`\[(?<linkText>[^\]\n]+)\]\((?<linkUrl>https?://[^\s()]+|/(?!/)[^\s()]*)\)`;

/** 生URL（`[text](url)` の外側で本文に直接書かれた URL）の自動リンク化。 */
const URL_SOURCE = String.raw`(?<url>https?://[^\s<>()\[\]]+)`;

/**
 * `@user` / `@user@host`（Fediverse形式）/ `@handle.bsky.social`（Bskyハンドル形式）。
 * 直前が英数字・アンダースコアの場合はメールアドレスの一部とみなしマッチしない。
 */
const MENTION_SOURCE = String.raw`(?<![\w])@(?<mention>[A-Za-z0-9_-]+(?:\.[A-Za-z0-9-]+)*(?:@[A-Za-z0-9.-]+)?)`;

/** 本文中のリッチテキスト要素をまとめて検出する結合正規表現（`RichText` 用）。 */
export const RICH_TEXT_SOURCE = [
  MARKDOWN_LINK_SOURCE,
  URL_SOURCE,
  MENTION_SOURCE,
  `(?<shortcode>${SHORTCODE_SOURCE})`,
].join("|");
