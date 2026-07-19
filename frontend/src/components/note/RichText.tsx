import { Link } from "react-router-dom";
import { RICH_TEXT_SOURCE, WORD_CHAR_RE } from "../../lib/richTextPatterns";
import styles from "./RichText.module.css";

interface RichTextProps {
  text: string;
  /** shortcode（`:name:`）→画像URLのマップ（`Note.emojis`）。未指定/空なら絵文字化しない。 */
  emojis?: Record<string, string>;
}

const RICH_TEXT_RE = new RegExp(RICH_TEXT_SOURCE, "gu");

/** カード全体のクリック（詳細遷移）へイベントが伝播しないようにする共通ハンドラ。 */
function stopPropagation(e: React.MouseEvent) {
  e.stopPropagation();
}

/**
 * 本文中の Markdownリンク `[text](url)`・生URL・`@mention`（Fediverse `@user@host` /
 * Bskyハンドル `@handle.bsky.social` の両形式）・絵文字ショートコードを1パスでトークナイズし、
 * クリック可能な要素へ変換する。`EmojiText` と同じ「マッチしなければただの文字列として残す」
 * 流儀（ラップ要素を持たず、呼び出し側の `<span>`/`<p>` のスタイルをそのまま活かす）。
 *
 * バックエンドは Bsky facet・AP `<a href>` を内部リンクマーカー `[text](url)` に変換して
 * `text` へ埋め込む（Misskey本家のMFMと同じ思想。API自体はプレーンテキストのままで互換性を
 * 保つ）。メンションはリンクマーカーで包まれず `@handle` 形式のまま届くため、このコンポーネント
 * 側で検出してプロフィールリンクへ変換する。
 *
 * セキュリティ: Markdownリンクの内部パス判定は `/` 始まり かつ `//` でないもの
 * （プロトコル相対URL `//evil.com/...` を内部リンク扱いにしないため）。`javascript:` 等の
 * 危険スキームは正規表現が `https?://` / `/` 以外を弾くため通らない。
 */
export default function RichText({ text, emojis }: RichTextProps) {
  const parts: React.ReactNode[] = [];
  let lastIndex = 0;
  let key = 0;
  const re = new RegExp(RICH_TEXT_RE);
  let match: RegExpExecArray | null;

  while ((match = re.exec(text)) !== null) {
    const g = match.groups!;

    if (g.linkUrl !== undefined) {
      if (match.index > lastIndex) parts.push(text.slice(lastIndex, match.index));
      const to = g.linkUrl;
      parts.push(
        to.startsWith("/") && !to.startsWith("//") ? (
          <Link key={key++} to={to} className={styles.link} onClick={stopPropagation}>
            {g.linkText}
          </Link>
        ) : (
          <a
            key={key++}
            href={to}
            target="_blank"
            rel="noopener noreferrer"
            className={styles.link}
            onClick={stopPropagation}
          >
            {g.linkText}
          </a>
        )
      );
    } else if (g.url !== undefined) {
      if (match.index > lastIndex) parts.push(text.slice(lastIndex, match.index));
      parts.push(
        <a
          key={key++}
          href={g.url}
          target="_blank"
          rel="noopener noreferrer"
          className={styles.link}
          onClick={stopPropagation}
        >
          {g.url}
        </a>
      );
    } else if (g.mention !== undefined) {
      if (match.index > lastIndex) parts.push(text.slice(lastIndex, match.index));
      parts.push(
        <Link key={key++} to={`/@${g.mention}`} className={styles.mention} onClick={stopPropagation}>
          @{g.mention}
        </Link>
      );
    } else if (g.shortcode !== undefined) {
      const shortcode = g.shortcode;
      const endIndex = match.index + shortcode.length;
      const nextChar = text[endIndex];
      if (nextChar && WORD_CHAR_RE.test(nextChar)) {
        continue;
      }
      const url = emojis?.[shortcode];
      if (!url) continue;
      if (match.index > lastIndex) parts.push(text.slice(lastIndex, match.index));
      parts.push(
        <img key={key++} className={styles.emojiImg} src={url} alt={shortcode} title={shortcode} loading="lazy" />
      );
    }

    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < text.length) {
    parts.push(text.slice(lastIndex));
  }
  return <>{parts}</>;
}
