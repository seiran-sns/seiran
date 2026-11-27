import { useTranslation } from "react-i18next";
import styles from "./StickySaveBar.module.css";

interface Props {
  saving: boolean;
  disabled?: boolean;
  /** 保存失敗時のメッセージ。 */
  error?: string;
  /** 保存できたときに出すメッセージ（未保存なら空）。 */
  savedMessage?: string;
  label?: string;
  savingLabel?: string;
}

/**
 * フォーム末尾に置く、画面下端に張り付く横幅いっぱいの保存ボタン。
 * 保存結果はボタンのすぐ上に出す（ボタンが下端固定なので、フォーム先頭に出すと見えない）。
 * 親フォームの左右パディングが 16px でなければ `--save-bar-gutter` で伝える。
 */
export default function StickySaveBar({ saving, disabled, error, savedMessage, label, savingLabel }: Props) {
  const { t } = useTranslation();
  return (
    <div className={styles.bar}>
      {error && <p className={styles.error}>{error}</p>}
      {savedMessage && <p className={styles.success}>{savedMessage}</p>}
      <button className={styles.button} type="submit" disabled={saving || disabled}>
        {saving ? (savingLabel ?? t("common:saving")) : (label ?? t("common:save"))}
      </button>
    </div>
  );
}
