import { RefObject, useMemo, useRef, useState } from "react";
import { blurhashToDataUrl } from "../../lib/blurhashPreview";
import { EmojiSpan } from "../../lib/emojiAspect";
import { useLazyVisible } from "../../hooks/useLazyVisible";
import styles from "./EmojiImage.module.css";

interface EmojiImageProps {
  src: string;
  alt: string;
  blurhash?: string;
  /** グリッド上での表示幅（1.5em単位、`emojiAspectSpan`の結果）。 */
  span?: EmojiSpan;
  /** 画像を実際にDOMへ描画するかどうかを判定するスクロールコンテナ（絵文字ピッカーの`.body`）。 */
  rootRef: RefObject<Element | null>;
}

/**
 * 絵文字画像。blurhashがあればフェッチ完了までプレースホルダとして表示し、本画像の
 * ロード完了と同時にプレースホルダをDOMから外して入れ替える。重ね描画ではなく除去する
 * ことで、本画像が透過部分を持っていてもプレースホルダの色が透けて見える事故を避ける。
 *
 * `rootRef` のスクロール範囲外にある間は img 要素自体をDOMから外す。カスタム絵文字が
 * 数千件になりうる一覧・検索結果表示で、一度に大量の画像リクエスト・デコードが走るのを防ぐ。
 */
export default function EmojiImage({ src, alt, blurhash, span = 1, rootRef }: EmojiImageProps) {
  const wrapRef = useRef<HTMLSpanElement>(null);
  const visible = useLazyVisible(wrapRef, rootRef);
  const [loaded, setLoaded] = useState(false);
  const placeholderUrl = useMemo(() => (blurhash ? blurhashToDataUrl(blurhash) : null), [blurhash]);

  return (
    <span ref={wrapRef} className={styles.wrap} style={{ width: `${span * 1.5}em` }}>
      {visible && !loaded && placeholderUrl && (
        <img className={styles.layer} src={placeholderUrl} alt="" aria-hidden="true" />
      )}
      {visible && (
        <img
          className={styles.layer}
          style={{ opacity: loaded ? 1 : 0 }}
          src={src}
          alt={alt}
          loading="lazy"
          onLoad={() => setLoaded(true)}
        />
      )}
    </span>
  );
}
