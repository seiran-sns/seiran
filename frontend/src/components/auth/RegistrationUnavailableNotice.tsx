import { useTranslation } from "react-i18next";
import { useSiteMeta } from "../../contexts/SiteMetaContext";
import styles from "../../pages/Auth.module.css";

/** 受付停止中、またはユーザー数上限に達していて新規登録・転入できない間、登録・転入フォームの
 * 代わりに出す案内。上限到達（`accountLimitReached`）より受付停止の表示を優先する。 */
export default function RegistrationUnavailableNotice() {
  const { t } = useTranslation();
  const { registrationClosed } = useSiteMeta();
  const code = registrationClosed ? "REGISTRATION_CLOSED" : "ACCOUNT_LIMIT_REACHED";
  return <p className={styles.error} style={{ textAlign: "center" }}>{t(`errors:${code}`)}</p>;
}
