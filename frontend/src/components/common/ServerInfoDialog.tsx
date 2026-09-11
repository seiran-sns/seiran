import { useTranslation } from "react-i18next";
import { useSiteMeta } from "../../contexts/SiteMetaContext";
import { FRONTEND_VERSION } from "../../version";
import Modal from "./Modal";
import styles from "./ServerInfoDialog.module.css";

const GITHUB_REPO_URL = "https://github.com/seiran-sns/seiran";
const GITHUB_ORG_LOGO_URL = "https://avatars.githubusercontent.com/u/293582574?v=4";

interface Props {
  open: boolean;
  onClose: () => void;
}

/** 左メニュー下部の「Powered by Seiran」から開く、プロジェクト情報ダイアログ。 */
export default function ServerInfoDialog({ open, onClose }: Props) {
  const { t } = useTranslation();
  const { serverVersion } = useSiteMeta();

  return (
    <Modal open={open} onClose={onClose} title={t("common:serverInfoDialog.title")}>
      <div className={styles.body}>
        <div className={styles.header}>
          <img src={GITHUB_ORG_LOGO_URL} alt="" className={styles.logo} />
          <span className={styles.name}>Seiran</span>
        </div>
        <a
          href={GITHUB_REPO_URL}
          target="_blank"
          rel="noopener noreferrer"
          className={styles.githubLink}
        >
          {t("common:serverInfoDialog.githubLink")}
        </a>
        <div className={styles.versions}>
          <div className={styles.versionRow}>
            <span className={styles.versionLabel}>{t("common:serverInfoDialog.frontendVersion")}</span>
            <span>{FRONTEND_VERSION}</span>
          </div>
          <div className={styles.versionRow}>
            <span className={styles.versionLabel}>{t("common:serverInfoDialog.serverVersion")}</span>
            <span>{serverVersion || "…"}</span>
          </div>
        </div>
      </div>
    </Modal>
  );
}
