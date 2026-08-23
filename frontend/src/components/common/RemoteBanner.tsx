import { useTranslation } from "react-i18next";
import blueskyLogo from "../../assets/bluesky-logo.svg";
import fediverseLogo from "../../assets/fediverse-logo.svg";
import styles from "./RemoteBanner.module.css";

interface RemoteBannerProps {
  /** バナー本文（例:「リモートのポストです」）。 */
  message: string;
  /** 元サーバー（Fedi）/ bsky.app（Bsky）上の URL。 */
  url: string;
  /** アイコンの出し分け（bsky以外はFediverseロゴ）。デフォルトはfedi。 */
  protocol?: "fedi" | "bsky";
}

/** ポスト詳細・プロフィールページ最上部に表示する「リモートで表示」バナー。 */
export default function RemoteBanner({ message, url, protocol = "fedi" }: RemoteBannerProps) {
  const { t } = useTranslation();
  return (
    <div className={styles.remoteBanner}>
      <img
        src={protocol === "bsky" ? blueskyLogo : fediverseLogo}
        alt=""
        className={styles.icon}
      />
      <span className={styles.message}>{message}</span>
      <a className={styles.link} href={url} target="_blank" rel="noopener noreferrer">
        {t("common:remoteBanner.viewRemote")} ↗
      </a>
    </div>
  );
}
