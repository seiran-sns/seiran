import { createElement, Fragment } from "react";
import { Link } from "react-router-dom";
import { ResolvedLinkInfo } from "../../api/types";
import { profilePath } from "../../lib/format";
import { MENTION_SOURCE, SHORTCODE_SOURCE, WORD_CHAR_RE } from "../../lib/richTextPatterns";
import { renderTextWithTwemoji } from "../../lib/twemoji";
import { mediaUrl } from "../../utils/mediaProxy";
import Avatar from "./Avatar";
import EmojiContextMenu from "./EmojiContextMenu";
import UserLinkTag from "./UserLinkTag";
import styles from "./RichText.module.css";

interface RichHtmlProps {
  /** バックエンドでサニタイズ済みのHTML（`Note.contentHtml`、またはプロフィールの
   * bio/profile_fields値を`toProfileHtml`でHTML化したもの）。 */
  html: string;
  /** shortcode（`:name:`）→画像URLのマップ（`Note.emojis`）。未指定/空なら絵文字化しない。 */
  emojis?: Record<string, string>;
  /** 外部URL(href文字列)→解決済みリンク情報（#リンク解決）。指定したURLの`<a>`は、解決先が
   * ユーザーならホバーでフォロースイッチ・右クリックで対ユーザー操作メニュー・先頭に顔アイコン
   * 付きのサイト内リンクへ、投稿なら通常のサイト内`<Link>`へ差し替える。`ProfilePage`のみが
   * 指定し、Note本文（`RichHtml`の元来の呼び出し元）は指定しないため常に従来通りの外部
   * `<a>`として描画される。 */
  resolvedLinks?: Record<string, ResolvedLinkInfo>;
}

/** テキストノード中の裸URL・メンション記法（`@user@host`/`@handle.bsky.social`）・絵文字
 * ショートコードを1パスでトークナイズするための結合正規表現（`RichText`の
 * `RICH_TEXT_SOURCE`と同じ考え方）。バックエンドがサニタイズ時にHTML化しない値
 * （Misskey等がメンションをリンクとして送らないprofile_fields値等）で、地の文のまま
 * 届いたURL・メンションも拾うために使う（#リンク解決）。 */
const URL_SOURCE = String.raw`https?://[^\s<>()[\]]+`;
const TEXT_TOKEN_RE = new RegExp(
  `(?<url>${URL_SOURCE})|${MENTION_SOURCE}|(?<shortcode>${SHORTCODE_SOURCE})`,
  "gu",
);
/** 既存の`<a>`タグの中のテキストで使う版（URL/メンション検出を含めない）。`<a>`の子孫テキスト
 * にまで裸URL/メンション検出をかけると、リンクの中にさらにリンクを作ってしまう
 * （`<a href="https://x.example/">https://x.example/</a>`のような、リンクテキスト自体が
 * URLと一致するケースで二重の`<a>`ネストになる不正なHTMLを生成する）。 */
const SHORTCODE_ONLY_RE = new RegExp(`(?<shortcode>${SHORTCODE_SOURCE})`, "gu");

/** バックエンドが `<a>` のメンション/ハッシュタグをこの形の内部パスへ書き換える
 * （`sanitize_ap_content_html`/`rewrite_mention_hashtag_hrefs` 参照）。 */
function isInternalPath(href: string): boolean {
  return href.startsWith("/@") || href.startsWith("/tags/");
}

/** リンクテキストが厳密に`@user`/`@user@host`/`@handle.bsky.social`形状のみで構成されている
 * か（前後に無関係な文字を含まない）。`<a href>`のhrefがリンクテキストと一致しない（例:
 * リモート側の実装がメンション表記を誤った形のURLへ組み立てて送ってくる）場合に、hrefより
 * リンクテキストの方を信頼できる解決対象として優先するために使う。 */
const FULL_MENTION_RE = /^@[A-Za-z0-9_-]+(?:\.[A-Za-z0-9-]+)*(?:@[A-Za-z0-9.-]+)?$/u;

/** 解決済み/未解決のリンク先文字列（URLまたは`@mention`）を、`resolvedLinks`の有無に応じて
 * リッチな内部リンク（解決済みユーザー）・サイト内リンク（解決済み投稿）・楽観的な内部リンク
 * （未解決メンション）・通常の外部リンク（未解決URL）のいずれかへ変換する。`<a href>`タグ由来
 * （`renderNode`）・テキストノード中の裸URL/メンション（`renderTextNode`）の両方で使う。
 * `candidates`は解決対象の候補を優先順に並べたもの（通常は`[href]`の1件のみ。`href`とリンク
 * テキストが食い違う`<a>`タグでは`[リンクテキスト, href]`のように複数渡し、いずれかが陽性
 * 解決されていればそちらを使う。フォールバック時の表示・遷移先は先頭の候補を使う）。 */
function renderLinkTarget(
  key: string,
  candidates: string[],
  label: React.ReactNode,
  resolvedLinks: Record<string, ResolvedLinkInfo> | undefined,
): React.ReactNode {
  const resolved = candidates.map((c) => resolvedLinks?.[c]).find((r) => r?.kind === "actor" || r?.kind === "post");
  const targetStr = candidates[0];
  if (resolved?.kind === "actor" && resolved.username) {
    const target = {
      username: resolved.username,
      domain: resolved.domain,
      reportLabel: `@${resolved.username}${resolved.domain ? `@${resolved.domain}` : ""}`,
    };
    return (
      <UserLinkTag key={key} target={target} to={profilePath(resolved.username, resolved.domain)} className={styles.link}>
        <span className={styles.resolvedLinkIcon}>
          <Avatar url={resolved.avatar_url} name={resolved.username} size={16} />
        </span>
        {label}
      </UserLinkTag>
    );
  }
  if (resolved?.kind === "post" && resolved.post_id) {
    return (
      <Link key={key} to={`/notes/${resolved.post_id}`} className={styles.link} onClick={stopPropagation}>
        {label}
      </Link>
    );
  }
  if (targetStr.startsWith("@")) {
    // メンション記法で、まだ解決結果（陽性）が届いていない状態。`RichText`の`@mention`即時
    // リンク化と同じく、実在確認を待たず楽観的に内部プロフィールリンクへ変換する（`targetStr`
    // 自体はURLではないため、外部リンク扱い(target="_blank")のフォールバックに落とすと
    // 不正なリンクになってしまう）。解決済みになった時点で上の分岐が先に一致するようになる。
    return (
      <Link key={key} to={`/${targetStr}`} className={styles.mention} onClick={stopPropagation}>
        {label}
      </Link>
    );
  }
  return (
    <a key={key} href={targetStr} target="_blank" rel="nofollow noopener noreferrer" className={styles.link} onClick={stopPropagation}>
      {label}
    </a>
  );
}

const STYLE_ATTR_TAGS = new Set([
  "p", "div", "b", "i", "s", "code", "pre", "blockquote", "ruby", "rt", "rp",
  "h1", "h2", "h3", "figure", "ul", "ol", "li", "small", "center",
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

/** テキストノード1個を、裸URL・メンション記法（解決済みなら`resolvedLinks`でリッチ化、
 * 未解決なら楽観的内部リンク/通常の外部リンク）・絵文字ショートコード→画像・Unicode絵文字→
 * twemoji画像まで変換したReactノード列にする。バックエンドがHTML化しない値（Misskey等が
 * メンションをリンクとして送らないprofile_fields値等）で地の文のまま届いたURL・メンションも
 * ここで拾う（#リンク解決）。 */
function renderTextNode(
  text: string,
  keyPrefix: string,
  emojis?: Record<string, string>,
  resolvedLinks?: Record<string, ResolvedLinkInfo>,
  insideAnchor?: boolean,
): React.ReactNode[] {
  const hasEmojis = !!emojis && Object.keys(emojis).length > 0;
  const parts: React.ReactNode[] = [];
  let lastIndex = 0;
  let key = 0;
  const re = new RegExp(insideAnchor ? SHORTCODE_ONLY_RE : TEXT_TOKEN_RE);
  let match: RegExpExecArray | null;

  const flushPlainText = (end: number) => {
    if (end > lastIndex) {
      parts.push(...renderTextWithTwemoji(text.slice(lastIndex, end), `${keyPrefix}-t${key++}`, styles.emojiImg));
    }
  };

  while ((match = re.exec(text)) !== null) {
    const g = match.groups!;
    if (g.shortcode !== undefined) {
      const shortcode = g.shortcode;
      const endIndex = match.index + shortcode.length;
      const nextChar = text[endIndex];
      if (!hasEmojis || (nextChar && WORD_CHAR_RE.test(nextChar))) continue;
      const url = emojis![shortcode];
      if (!url) continue;
      flushPlainText(match.index);
      parts.push(
        <EmojiContextMenu key={`${keyPrefix}-e${key++}`} shortcode={shortcode.slice(1, -1)} imageUrl={url}>
          <img className={styles.emojiImg} src={mediaUrl(url)} alt={shortcode} title={shortcode} loading="lazy" />
        </EmojiContextMenu>
      );
      lastIndex = endIndex;
      continue;
    }
    const targetStr = g.url !== undefined ? g.url : `@${g.mention}`;
    flushPlainText(match.index);
    parts.push(renderLinkTarget(`${keyPrefix}-l${key++}`, [targetStr], targetStr, resolvedLinks));
    lastIndex = match.index + match[0].length;
  }
  flushPlainText(text.length);
  return parts;
}

/** バックエンドで既にサニタイズ済みのタグのみを対象に、パース済みDOMツリーをReact要素へ
 * 変換する（`dangerouslySetInnerHTML`を使わない多層防御。許可タグ外は子要素だけ描画する）。 */
function renderNode(
  node: ChildNode,
  keyPrefix: string,
  emojis?: Record<string, string>,
  resolvedLinks?: Record<string, ResolvedLinkInfo>,
  insideAnchor?: boolean,
): React.ReactNode {
  if (node.nodeType === Node.TEXT_NODE) {
    return renderTextNode(node.textContent ?? "", keyPrefix, emojis, resolvedLinks, insideAnchor);
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return null;

  const el = node as Element;
  const tag = el.tagName.toLowerCase();

  if (tag === "br") return <br key={keyPrefix} />;

  const childInsideAnchor = insideAnchor || tag === "a";
  const children = Array.from(el.childNodes).map((child, i) =>
    renderNode(child, `${keyPrefix}-${i}`, emojis, resolvedLinks, childInsideAnchor),
  );

  if (tag === "a") {
    const href = el.getAttribute("href") ?? "";
    if (isInternalPath(href)) {
      return (
        <Link key={keyPrefix} to={href} className={styles.mention} onClick={stopPropagation}>
          {children}
        </Link>
      );
    }
    // リンクテキストが`@user@host`形状そのものの場合、hrefより優先して解決を試みる
    // （リモート実装がメンション表記を誤った形のURLへ組み立てて送ってくることがあり、
    // その場合hrefは解決されずリンクテキストの方だけが解決されている、実機で確認済み）。
    const linkText = el.textContent?.trim() ?? "";
    const candidates = FULL_MENTION_RE.test(linkText) && linkText !== href ? [linkText, href] : [href];
    return renderLinkTarget(keyPrefix, candidates, children, resolvedLinks);
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
    const className = tag === "blockquote" ? styles.blockquote : tag === "pre" ? styles.pre : undefined;
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
export default function RichHtml({ html, emojis, resolvedLinks }: RichHtmlProps) {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const nodes = Array.from(doc.body.childNodes).map((node, i) =>
    renderNode(node, `n${i}`, emojis, resolvedLinks),
  );
  return <>{nodes}</>;
}
