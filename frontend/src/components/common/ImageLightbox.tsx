import { TouchEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./ImageLightbox.module.css";

interface ImageLightboxProps {
  /** 表示中の画像URL。null なら非表示。 */
  src: string | null;
  onClose: () => void;
  sensitive?: boolean;
  onPrevious?: () => void;
  onNext?: () => void;
}

const SWIPE_THRESHOLD_PX = 50;

/**
 * 添付画像クリック時のページ内ライトボックス表示（#64, #153）。
 *
 * オーバーレイ・Escで閉じ、複数画像ではボタン・左右矢印キー・スワイプで移動する。
 * 表示画像はアップロード時の最大サイズに合わせ、CSS上で1024×1024pxを上限とする。
 */
export default function ImageLightbox({
  src,
  onClose,
  sensitive = false,
  onPrevious,
  onNext,
}: ImageLightboxProps) {
  const { t } = useTranslation();
  const [revealed, setRevealed] = useState(false);
  const touchStartX = useRef<number | null>(null);

  useEffect(() => {
    if (!src) return;
    setRevealed(false);

    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        onClose();
      } else if (e.key === "ArrowLeft" && onPrevious) {
        e.preventDefault();
        onPrevious();
      } else if (e.key === "ArrowRight" && onNext) {
        e.preventDefault();
        onNext();
      }
    }

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [src, onClose, onPrevious, onNext]);

  if (!src) return null;

  function handleTouchStart(e: TouchEvent) {
    e.stopPropagation();
    touchStartX.current = e.changedTouches[0]?.clientX ?? null;
  }

  function handleTouchEnd(e: TouchEvent) {
    e.stopPropagation();
    if (touchStartX.current === null) return;
    const endX = e.changedTouches[0]?.clientX;
    if (endX === undefined) return;
    const delta = endX - touchStartX.current;
    touchStartX.current = null;

    if (delta > SWIPE_THRESHOLD_PX) onPrevious?.();
    if (delta < -SWIPE_THRESHOLD_PX) onNext?.();
  }

  return (
    <div
      className={styles.overlay}
      onClick={onClose}
      onTouchStart={(e) => e.stopPropagation()}
      onTouchEnd={(e) => e.stopPropagation()}
    >
      <button className={styles.close} onClick={onClose} aria-label={t("common:close")}>
        ×
      </button>
      {onPrevious && (
        <button
          className={`${styles.pageButton} ${styles.previous}`}
          onClick={(e) => {
            e.stopPropagation();
            onPrevious();
          }}
          aria-label={t("common:previousImage")}
        >
          ‹
        </button>
      )}
      <div
        className={styles.imageWrap}
        onClick={onClose}
        onTouchStart={handleTouchStart}
        onTouchEnd={handleTouchEnd}
      >
        <img
          src={src}
          alt=""
          className={`${styles.image} ${sensitive && !revealed ? styles.blurred : ""}`}
        />
        {sensitive && !revealed && (
          <button
            className={styles.reveal}
            aria-label={t("common:sensitiveImageReveal")}
            onClick={(e) => {
              e.stopPropagation();
              setRevealed(true);
            }}
          >
            👁️
          </button>
        )}
      </div>
      {onNext && (
        <button
          className={`${styles.pageButton} ${styles.next}`}
          onClick={(e) => {
            e.stopPropagation();
            onNext();
          }}
          aria-label={t("common:nextImage")}
        >
          ›
        </button>
      )}
    </div>
  );
}
