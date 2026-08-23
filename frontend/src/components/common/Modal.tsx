import { ReactNode, useEffect } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import styles from "./Modal.module.css";

interface ModalProps {
  open: boolean;
  onClose: () => void;
  title?: string;
  children: ReactNode;
}

/** ダイアログ駆動 UI の基盤モーダル（オーバーレイクリック・Esc で閉じる）。 */
export default function Modal({ open, onClose, title, children }: ModalProps) {
  const { t } = useTranslation();
  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  // NoteCard内の右クリックメニュー等、深くネストした場所からも開かれる。祖先の transform 等が
  // 作るスタッキングコンテキストに閉じ込められると z-index が効かず、他のポップオーバーの裏に
  // 隠れたり逆にヘッダー等の上に埋もれずかぶさったりするため、body 直下へポータルして描画する。
  return createPortal(
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.dialog} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          {title && <span className={styles.title}>{title}</span>}
          <button className={styles.close} onClick={onClose} aria-label={t("common:close")}>
            ×
          </button>
        </div>
        <div className={styles.body}>{children}</div>
      </div>
    </div>,
    document.body,
  );
}
