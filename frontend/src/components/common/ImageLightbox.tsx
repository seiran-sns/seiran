import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./ImageLightbox.module.css";

interface ImageLightboxProps {
  /** 表示中の画像URL。null なら非表示。 */
  src: string | null;
  onClose: () => void;
  sensitive?: boolean;
}

/**
 * 添付画像クリック時のページ内ライトボックス表示（#64）。
 *
 * 添付画像アップロード時の最大リサイズは 2048×2048px（`storage/image.rs`）で、これは
 * Retina（2倍密度）ディスプレイでの実質 1024 CSS px 相当の表示を見込んだサイズのため、
 * ライトボックスの最大表示サイズも CSS 上で 1024×1024px（アスペクト比維持）とする。
 *
 * オーバーレイクリック・Esc キーで閉じる操作性は `Modal` を踏襲しつつ、
 * 画像ビューアとしてタイトルバー・枠のないボーダーレスな見た目にするため専用コンポーネントとする。
 */
export default function ImageLightbox({ src, onClose, sensitive = false }: ImageLightboxProps) {
  const { t } = useTranslation();
  const [revealed, setRevealed] = useState(false);

  useEffect(() => {
    if (!src) return;
    setRevealed(false);
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [src, onClose]);

  if (!src) return null;

  return (
    <div className={styles.overlay} onClick={onClose}>
      <button className={styles.close} onClick={onClose} aria-label={t("common:close")}>
        ×
      </button>
      <div className={styles.imageWrap} onClick={(e) => e.stopPropagation()}>
        <img
          src={src}
          alt=""
          className={`${styles.image} ${sensitive && !revealed ? styles.blurred : ""}`}
        />
        {sensitive && !revealed && (
          <button className={styles.reveal} aria-label="閲覧注意画像を表示" onClick={(e) => {
            e.stopPropagation();
            setRevealed(true);
          }}>👁️</button>
        )}
      </div>
    </div>
  );
}
