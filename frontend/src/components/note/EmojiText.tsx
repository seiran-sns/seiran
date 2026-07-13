import styles from "./EmojiText.module.css";

const SHORTCODE_RE = /:[a-zA-Z0-9_+-]+:/g;

interface EmojiTextProps {
  text: string;
  /** shortcode（`:name:`）→画像URLのマップ（`Note.emojis`）。未指定/空ならプレーンテキストのまま。 */
  emojis?: Record<string, string>;
}

/** 英数字・アンダースコア（ショートコードの構成文字と同じ集合）。右端の接触判定に使う。 */
const WORD_CHAR_RE = /[a-zA-Z0-9_]/;

/**
 * 本文・表示名中の `:shortcode:` を、`emojis` マップで解決できるものだけ画像に置換する。
 * 解決できないショートコード（マップに無い・単なるコロン記法）はそのままテキスト表示する。
 * ラップ要素を持たない（呼び出し側の `<span>`/`<p>` 等のスタイルをそのまま活かすため）。
 *
 * 境界条件: 左端は何が接触していても良い（"わこつ:blobcatwave:" 等）。右端だけは
 * 英数字・アンダースコアが接触していないこと（"file:name_here:12345" のような無関係な
 * コロン記法を誤って絵文字化しないため）。
 */
export default function EmojiText({ text, emojis }: EmojiTextProps) {
  if (!emojis || Object.keys(emojis).length === 0) {
    return <>{text}</>;
  }

  const parts: React.ReactNode[] = [];
  let lastIndex = 0;
  let key = 0;
  const re = new RegExp(SHORTCODE_RE);
  let match: RegExpExecArray | null;
  while ((match = re.exec(text)) !== null) {
    const shortcode = match[0];
    const endIndex = match.index + shortcode.length;
    const nextChar = text[endIndex];
    if (nextChar && WORD_CHAR_RE.test(nextChar)) {
      continue;
    }
    const url = emojis[shortcode];
    if (!url) continue;
    if (match.index > lastIndex) {
      parts.push(text.slice(lastIndex, match.index));
    }
    parts.push(
      <img key={key++} className={styles.emojiImg} src={url} alt={shortcode} title={shortcode} loading="lazy" />
    );
    lastIndex = endIndex;
  }
  if (lastIndex < text.length) {
    parts.push(text.slice(lastIndex));
  }
  return <>{parts}</>;
}
