import { useState } from "react";

interface TwemojiImgProps {
  /** 元のUnicode絵文字（フォールバック表示・alt用）。 */
  emoji: string;
  /** セルフホストtwemoji SVGのURL。 */
  url: string;
  /** 呼び出し側が組み立て済みの最終クラス名をそのまま渡すこと（このコンポーネント自身は
   * 既定クラスを補わない。呼び出し元ごとにサイズ指定クラスの組み立て方が異なるため）。 */
  className?: string;
  /** 低コントラストモードでも色を落とさない（警告バッジ等、色そのものが意味を持つもの）。 */
  keepColor?: boolean;
}

/**
 * twemoji SVGを表示し、読み込みに失敗したら（`@twemoji/parser`は認識したが
 * `@twemoji/svg`に対応アセットが無い新しい絵文字等、パッケージ間のUnicode
 * カバレッジのズレにより発生し得る）OSネイティブの絵文字グリフへフォールバックする。
 * `TwemojiEmoji`・`renderTextWithTwemoji`（`lib/twemoji.tsx`）の両方から共用する。
 */
export default function TwemojiImg({ emoji, url, className, keepColor = false }: TwemojiImgProps) {
  const [failed, setFailed] = useState(false);

  if (failed) return <>{emoji}</>;

  return (
    <img
      className={className}
      src={url}
      alt={emoji}
      draggable={false}
      data-emoji={keepColor ? undefined : ""}
      loading="lazy"
      onError={() => setFailed(true)}
    />
  );
}
