/**
 * 管理者が書く HTML 可のテキスト（利用規約）は `white-space: pre-wrap` で表示し、タグを
 * 使わない改行もそのまま改行として出す。ただしそのままだと、ブロック要素のタグの前後に
 * ソース上で入れた改行まで空行として出て、見出しや段落の間が大きく空く。そこで、
 * ブロック要素の境目に接する改行を1個だけ取り除く。
 *
 * - ブロック要素の開始タグ・終了タグの直前と直後（要素内の先頭・末尾も含む）が対象。`<br>` も
 *   それ自体が改行なので、ブロック要素と同じに扱う（`<br>` と改行を両方書いて2回改行されないように）。
 * - 境目ごとに取り除くのは1個だけ。改行を2個以上続けて書けば、残った分が空行として出る。
 * - タグとタグの間が空白と改行だけなら、その間は1つの境目として扱う（改行1個を取り除き、
 *   改行が残らなければ空白も取り除く。インデントだけが残ると pre-wrap で余計な行になるため）。
 */

const BLOCK_TAGS = new Set([
  "address", "article", "aside", "blockquote", "caption", "dd", "details", "dialog", "div",
  "dl", "dt", "fieldset", "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4",
  "h5", "h6", "header", "hgroup", "hr", "li", "main", "nav", "ol", "p", "pre", "section",
  "summary", "table", "tbody", "td", "tfoot", "th", "thead", "tr", "ul",
  // 表示されないが、前後の改行は同じく空行になる
  "style", "script", "template",
  // それ自体が改行になる
  "br",
]);

const TAG_PATTERN = /<!--[\s\S]*?-->|<\/?([a-zA-Z][a-zA-Z0-9-]*)\b[^>]*>/g;

function isBlockTag(tagName: string | undefined): boolean {
  return tagName !== undefined && BLOCK_TAGS.has(tagName.toLowerCase());
}

/** `text`（タグとタグの間の文字列）から、両隣のタグに応じて改行を取り除く。 */
function trimGap(text: string, afterBlock: boolean, beforeBlock: boolean): string {
  if (!afterBlock && !beforeBlock) return text;
  if (text.trim() === "") {
    const rest = text.replace(/[ \t]*\r?\n/, "");
    return /\n/.test(rest) ? rest : "";
  }
  let out = text;
  if (afterBlock) out = out.replace(/^[ \t]*\r?\n/, "");
  if (beforeBlock) out = out.replace(/\r?\n[ \t]*$/, "");
  return out;
}

export function trimBlockBoundaryNewlines(html: string): string {
  let out = "";
  let last = 0;
  let prevIsBlock = false;
  for (const match of html.matchAll(TAG_PATTERN)) {
    const index = match.index ?? 0;
    // コメントは境目の判定に影響させない（前のタグの種類を引き継ぐ）
    const isComment = match[0].startsWith("<!--");
    const isBlock = !isComment && isBlockTag(match[1]);
    out += trimGap(html.slice(last, index), prevIsBlock, isBlock) + match[0];
    if (!isComment) prevIsBlock = isBlock;
    last = index + match[0].length;
  }
  return out + trimGap(html.slice(last), prevIsBlock, false);
}
