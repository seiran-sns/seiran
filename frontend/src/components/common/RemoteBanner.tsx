import { useTranslation } from "react-i18next";
import styles from "./RemoteBanner.module.css";

interface RemoteBannerProps {
  /** バナー本文（例:「リモートのポストです」）。 */
  message: string;
  /** 元サーバー（Fedi）/ bsky.app（Bsky）上の URL。 */
  url: string;
}

/** ポスト詳細・プロフィールページ最上部に表示する「リモートで表示」バナー。 */
export default function RemoteBanner({ message, url }: RemoteBannerProps) {
  const { t } = useTranslation();
  return (
    <div className={styles.remoteBanner}>
      <span className={styles.icon}>🌐</span>
      <span className={styles.message}>{message}</span>
      <a className={styles.link} href={url} target="_blank" rel="noopener noreferrer">
        {t("common:remoteBanner.viewRemote")} ↗
      </a>
    </div>
  );
}
