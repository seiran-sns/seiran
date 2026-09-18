import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAuth } from "../../contexts/AuthContext";
import Modal from "./Modal";
import styles from "./ServerUnavailableDialog.module.css";

/**
 * バックエンド停止中等で`/auth/me`が解決できない（`AuthContext`の`sessionUnresolved`）間、
 * ログイン画面へ誤って遷移させる代わりに表示するダイアログ。アプリのルートで一度だけマウントする。
 */
export default function ServerUnavailableDialog() {
  const { t } = useTranslation();
  const { sessionUnresolved } = useAuth();
  const [dismissed, setDismissed] = useState(false);

  // 再度未解決状態になったら、前回閉じた記録をリセットして再表示する。
  useEffect(() => {
    if (sessionUnresolved) setDismissed(false);
  }, [sessionUnresolved]);

  const open = sessionUnresolved && !dismissed;

  return (
    <Modal open={open} onClose={() => setDismissed(true)} title={t("common:serverUnavailable.title")}>
      <p className={styles.body}>{t("common:serverUnavailable.body")}</p>
    </Modal>
  );
}
