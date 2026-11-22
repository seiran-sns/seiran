import { useTranslation } from "react-i18next";
import Modal from "../common/Modal";
import { useSiteMeta } from "../../contexts/SiteMetaContext";

interface MigrationInfoDialogProps {
  open: boolean;
  onClose: () => void;
  /** 希望のユーザー名（入力前なら空）。転入後のハンドルの見本に使う。 */
  username: string;
}

/**
 * 「ℹ️転入とは？」のダイアログ。Bluesky のアカウントがサーバー（PDS）間で引っ越せること、
 * 転入すると元のサーバーではログインできなくなること、フォロー関係やリプライのスレッド・
 * メンションは保たれること、ハンドルが `@<ユーザー名>.<このサーバーのドメイン>` に変わることを
 * 説明する（元のアカウントが無効化される仕組みが伝わっていなかったため、転入の入口に置く）。
 */
export default function MigrationInfoDialog({ open, onClose, username }: MigrationInfoDialogProps) {
  const { t } = useTranslation();
  const { localDomain } = useSiteMeta();
  const domain = localDomain || window.location.host;
  const handle = `${username.trim().toLowerCase() || "****"}.${domain}`;

  return (
    <Modal open={open} onClose={onClose} title={t("auth:migrationInfo.title")}>
      <ul style={{ margin: 0, paddingLeft: "1.2em", lineHeight: 1.7 }}>
        <li>{t("auth:migrationInfo.pdsMove", { domain })}</li>
        <li>
          <strong>{t("auth:migrationInfo.loginDisabled", { domain })}</strong>
        </li>
        <li>{t("auth:migrationInfo.relationsKept")}</li>
        <li>{t("auth:migrationInfo.handleChange", { handle })}</li>
      </ul>
    </Modal>
  );
}
