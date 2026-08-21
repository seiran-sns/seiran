import { createElement, Fragment } from "react";
import { Link } from "react-router-dom";
import { SHORTCODE_SOURCE, WORD_CHAR_RE } from "../../lib/richTextPatterns";
import { renderTextWithTwemoji } from "../../lib/twemoji";
import { mediaUrl } from "../../utils/mediaProxy";
import EmojiContextMenu from "./EmojiContextMenu";
import styles from "./RichText.module.css";

interface RichHtmlProps {
  /** バックエンドでサニタイズ済みのHTML（`Note.contentHtml`）。 */
  html: string;
  /** shortcode（`:name:`）→画像URLのマップ（`Note.emojis`）。未指定/空なら絵文字化しない。 */
  emojis?: Record<string, string>;
}

const SHORTCODE_RE = new RegExp(SHORTCODE_SOURCE, "gu");

/** バックエンドが `<a>` のメンション/ハッシュタグをこの形の内部パスへ書き換える
 * （`sanitize_ap_content_html`/`rewrite_mention_hashtag_hrefs` 参照）。 */
function isInternalPath(href: string): boolean {
  return href.startsWith("/@") || href.startsWith("/tags/");
}

const STYLE_ATTR_TAGS = new Set([
  "p", "div", "b", "i", "s", "code", "pre", "blockquote", "ruby", "rt", "rp",
  "h1", "h2", "figure", "ul", "ol", "li", "small", "center",
]);

function textAlignFromStyleAttr(style: string | null): React.CSSProperties | undefined {
  if (!style) return undefined;
  const m = /^text-align:\s*(left|right|center|justify)$/.exec(style.trim());
  return m ? { textAlign: m[1] as React.CSSProperties["textAlign"] } : undefined;
}

/** カード全体のクリック（詳細遷移）へイベントが伝播しないようにする共通ハンドラ。 */
function stopPropagation(e: React.MouseEvent) {
  e.stopPropagation();
}

/** テキストノード1個を、絵文字ショートコード→画像・Unicode絵文字→twemoji画像まで変換した
 * Reactノード列にする（`EmojiText`/`RichText`と同じ変換ロジック）。 */
function renderTextNode(text: string, keyPrefix: string, emojis?: Record<string, string>): React.ReactNode[] {
  const rawParts: React.ReactNode[] = [];
  if (emojis && Object.keys(emojis).length > 0) {
    let lastIndex = 0;
    let key = 0;
    const re = new RegExp(SHORTCODE_RE);
    let match: RegExpExecArray | null;
    while ((match = re.exec(text)) !== null) {
      const shortcode = match[0];
      const endIndex = match.index + shortcode.length;
      const nextChar = text[endIndex];
      if (nextChar && WORD_CHAR_RE.test(nextChar)) continue;
      const url = emojis[shortcode];
      if (!url) continue;
      if (match.index > lastIndex) rawParts.push(text.slice(lastIndex, match.index));
      rawParts.push(
        <EmojiContextMenu key={`${keyPrefix}-e${key++}`} shortcode={shortcode.slice(1, -1)} imageUrl={url}>
          <img className={styles.emojiImg} src={mediaUrl(url)} alt={shortcode} title={shortcode} loading="lazy" />
        </EmojiContextMenu>
      );
      lastIndex = endIndex;
    }
    if (lastIndex < text.length) rawParts.push(text.slice(lastIndex));
  } else {
    rawParts.push(text);
  }
  return rawParts.flatMap((part, i) =>
    typeof part === "string" ? renderTextWithTwemoji(part, `${keyPrefix}-t${i}`, styles.emojiImg) : [part]
  );
}

/** バックエンドで既にサニタイズ済みのタグのみを対象に、パース済みDOMツリーをReact要素へ
 * 変換する（`dangerouslySetInnerHTML`を使わない多層防御。許可タグ外は子要素だけ描画する）。 */
function renderNode(node: ChildNode, keyPrefix: string, emojis?: Record<string, string>): React.ReactNode {
  if (node.nodeType === Node.TEXT_NODE) {
    return renderTextNode(node.textContent ?? "", keyPrefix, emojis);
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return null;

  const el = node as Element;
  const tag = el.tagName.toLowerCase();
  const children = Array.from(el.childNodes).map((child, i) => renderNode(child, `${keyPrefix}-${i}`, emojis));

  if (tag === "br") return <br key={keyPrefix} />;

  if (tag === "a") {
    const href = el.getAttribute("href") ?? "";
    if (isInternalPath(href)) {
      return (
        <Link key={keyPrefix} to={href} className={styles.mention} onClick={stopPropagation}>
          {children}
        </Link>
      );
    }
    return (
      <a key={keyPrefix} href={href} target="_blank" rel="nofollow noopener noreferrer" className={styles.link} onClick={stopPropagation}>
        {children}
      </a>
    );
  }

  if (tag === "img") {
    return (
      <img
        key={keyPrefix}
        src={mediaUrl(el.getAttribute("src"))}
        alt={el.getAttribute("alt") ?? ""}
        width={el.getAttribute("width") ?? undefined}
        height={el.getAttribute("height") ?? undefined}
        loading="lazy"
      />
    );
  }

  if (STYLE_ATTR_TAGS.has(tag)) {
    const style = textAlignFromStyleAttr(el.getAttribute("style"));
    const className = tag === "blockquote" ? styles.blockquote : undefined;
    return createElement(tag, { key: keyPrefix, style, className }, ...children);
  }

  // 許可タグ外（多層防御、通常はバックエンドのサニタイズで既に除去済み）: タグは描画せず
  // 子要素だけ残す。
  return <Fragment key={keyPrefix}>{children}</Fragment>;
}

/**
 * `Note.contentHtml`（バックエンドでallowlistサニタイズ済みのHTML）をReact要素として描画する。
 * `RichText`（`Note.text`のプレーンテキストもどきをパースする版）とは別に、リモートFedi投稿の
 * `<blockquote>`/`<ruby>`等の意味的構造を保持したまま表示するために使う。`contentHtml`が
 * 無い投稿（ローカル投稿・Bsky投稿・移行前の既存投稿）は`RichText`側にフォールバックする
 * （呼び出し側で分岐、`NoteCard`参照）。
 */
export default function RichHtml({ html, emojis }: RichHtmlProps) {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const nodes = Array.from(doc.body.childNodes).map((node, i) => renderNode(node, `n${i}`, emojis));
  return <>{nodes}</>;
}
