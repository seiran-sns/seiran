import { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import Modal from "./Modal";
import styles from "./ConfirmModal.module.css";

interface ConfirmModalProps {
  open: boolean;
  onClose: () => void;
  title: string;
  /** 確認の本文。選択欄など操作の中身もここに置く。 */
  children: ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
  /** 実行中（確定ボタンを押せなくする）。 */
  busy?: boolean;
}

/** 凍結・ロール変更など取り返しのつきにくい操作の前に出す確認ダイアログ。 */
export default function ConfirmModal({
  open,
  onClose,
  title,
  children,
  confirmLabel,
  onConfirm,
  busy,
}: ConfirmModalProps) {
  const { t } = useTranslation();
  return (
    <Modal open={open} onClose={onClose} title={title}>
      <div className={styles.body}>{children}</div>
      <div className={styles.actions}>
        <button className={styles.primary} onClick={onConfirm} disabled={busy}>
          {confirmLabel}
        </button>
        <button className={styles.secondary} onClick={onClose}>
          {t("common:cancel")}
        </button>
      </div>
    </Modal>
  );
}
