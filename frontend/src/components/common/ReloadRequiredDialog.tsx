import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { dismissReloadRequired, setReloadRequiredHandler } from "../../api/versionCompat";
import Modal from "./Modal";
import styles from "./ReloadRequiredDialog.module.css";

/**
 * フロントエンドとサーバーのバージョン互換性チェック（`api/versionCompat.ts`）に
 * 引っかかった際、リロードを促すダイアログ。アプリのルートで一度だけマウントする。
 */
export default function ReloadRequiredDialog() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    setReloadRequiredHandler(() => setOpen(true));
    return () => setReloadRequiredHandler(null);
  }, []);

  function handleClose() {
    setOpen(false);
    dismissReloadRequired();
  }

  return (
    <Modal open={open} onClose={handleClose} title={t("common:reloadRequired.title")}>
      <p className={styles.body}>{t("common:reloadRequired.body")}</p>
      <div className={styles.actions}>
        <button
          type="button"
          className={styles.primary}
          onClick={() => window.location.reload()}
        >
          {t("common:reloadRequired.reload")}
        </button>
      </div>
    </Modal>
  );
}
