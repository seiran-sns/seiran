import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import blueskyLogo from "../../assets/bluesky-logo.svg";
import fediverseLogo from "../../assets/fediverse-logo.svg";
import styles from "./RemoteBanner.module.css";

interface RemoteBannerProps {
  /** バナー本文（例:「リモートのポストです」）。 */
  message: string;
  /** 元サーバー（Fedi）/ bsky.app（Bsky）上の URL。`internal`指定時はseiran内部の相対パス。 */
  url: string;
  /** アイコンの出し分け（bsky以外はFediverseロゴ）。デフォルトはfedi。`internal`指定時は無視。 */
  protocol?: "fedi" | "bsky";
  /** リンクのラベル文言。省略時は「リモートで表示」。 */
  linkLabel?: string;
  /** `true`ならseiran内部の投稿詳細ページへのSPA内遷移として扱う（新規タブを開く外部リンク
   * 用の「↗」サフィックス・アイコンを出さない、ブリッジポストの「元ポストを表示」用）。 */
  internal?: boolean;
}

/** ポスト詳細・プロフィールページ最上部に表示する「リモートで表示」バナー。 */
export default function RemoteBanner({
  message,
  url,
  protocol = "fedi",
  linkLabel,
  internal = false,
}: RemoteBannerProps) {
  const { t } = useTranslation();
  const label = linkLabel ?? t("common:remoteBanner.viewRemote");
  return (
    <div className={styles.remoteBanner}>
      {!internal && (
        <img
          src={protocol === "bsky" ? blueskyLogo : fediverseLogo}
          alt=""
          className={styles.icon}
        />
      )}
      <span className={styles.message}>{message}</span>
      {internal ? (
        <Link className={styles.link} to={url}>
          {label}
        </Link>
      ) : (
        <a className={styles.link} href={url} target="_blank" rel="noopener noreferrer">
          {label} ↗
        </a>
      )}
    </div>
  );
}
