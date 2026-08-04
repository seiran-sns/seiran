import { renderTextWithTwemoji } from "../../lib/twemoji";
import styles from "./TwemojiEmoji.module.css";

interface TwemojiTextProps {
  text: string;
  className?: string;
}

/**
 * 絵文字混在テキスト（"💬 返信" のようなラベル等）のUnicode絵文字部分だけをセルフホストの
 * twemoji SVGに変換して表示する。カスタム絵文字ショートコードも扱いたい場合は
 * `components/note/EmojiText` を使うこと。
 */
export default function TwemojiText({ text, className }: TwemojiTextProps) {
  const imgClassName = className ? `${styles.emoji} ${className}` : styles.emoji;
  return <>{renderTextWithTwemoji(text, "twt", imgClassName)}</>;
}
