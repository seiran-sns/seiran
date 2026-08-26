import { useMemo, useState } from "react";
import { blurhashToDataUrl } from "../../lib/blurhashPreview";
import { EmojiSpan } from "../../lib/emojiAspect";
import styles from "./EmojiImage.module.css";

interface EmojiImageProps {
  src: string;
  alt: string;
  blurhash?: string;
  /** グリッド上での表示幅（1.5em単位、`emojiAspectSpan`の結果）。 */
  span?: EmojiSpan;
}

/**
 * 絵文字画像。blurhashがあればフェッチ完了までプレースホルダとして表示し、本画像の
 * ロード完了と同時にプレースホルダをDOMから外して入れ替える。重ね描画ではなく除去する
 * ことで、本画像が透過部分を持っていてもプレースホルダの色が透けて見える事故を避ける。
 */
export default function EmojiImage({ src, alt, blurhash, span = 1 }: EmojiImageProps) {
  const [loaded, setLoaded] = useState(false);
  const placeholderUrl = useMemo(() => (blurhash ? blurhashToDataUrl(blurhash) : null), [blurhash]);

  return (
    <span className={styles.wrap} style={{ width: `${span * 1.5}em` }}>
      {!loaded && placeholderUrl && (
        <img className={styles.layer} src={placeholderUrl} alt="" aria-hidden="true" />
      )}
      <img
        className={styles.layer}
        style={{ opacity: loaded ? 1 : 0 }}
        src={src}
        alt={alt}
        loading="lazy"
        onLoad={() => setLoaded(true)}
      />
    </span>
  );
}
