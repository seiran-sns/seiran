import { singleEmojiToTwemojiUrl } from "../../lib/twemoji";
import styles from "./TwemojiEmoji.module.css";

interface TwemojiEmojiProps {
  /** 単一のUnicode絵文字文字列（サロゲートペア込み）。 */
  emoji: string;
  className?: string;
}

/**
 * 単一のUnicode絵文字をセルフホストのtwemoji SVGへ変換して表示する。
 * OS/ブラウザ依存のネイティブ絵文字グリフではなく統一された見た目にするための共通部品。
 * Unicode絵文字として認識できない文字列（カスタム絵文字ショートコード等）はそのままテキスト表示する。
 */
export default function TwemojiEmoji({ emoji, className }: TwemojiEmojiProps) {
  const url = singleEmojiToTwemojiUrl(emoji);
  if (!url) return <>{emoji}</>;
  return (
    <img
      className={className ? `${styles.emoji} ${className}` : styles.emoji}
      src={url}
      alt={emoji}
      draggable={false}
      loading="lazy"
    />
  );
}
