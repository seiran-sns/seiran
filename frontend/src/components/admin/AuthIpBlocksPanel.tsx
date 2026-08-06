import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, AuthIpBlock, getErrorMessage } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

export default function AuthIpBlocksPanel() {
  const { t, i18n } = useTranslation();
  const [blocks, setBlocks] = useState<AuthIpBlock[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyIp, setBusyIp] = useState<string | null>(null);
  const [error, setError] = useState("");

  const load = () => {
    setLoading(true);
    api.admin
      .listAuthIpBlocks()
      .then(setBlocks)
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  };

  useEffect(load, []);

  async function unblock(ip: string) {
    if (!confirm(t("admin:authIpBlocksPanel.unblockConfirm", { ip }))) return;
    setBusyIp(ip);
    setError("");
    try {
      await api.admin.unblockAuthIp(ip);
      setBlocks((current) => current.filter((b) => b.ip_address !== ip));
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusyIp(null);
    }
  }

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:authIpBlocksPanel.title")}</h2>
      <p className={styles.hint}>{t("admin:authIpBlocksPanel.description")}</p>
      {error && <p className={styles.error}>{error}</p>}
      {loading ? (
        <p className={panel.message}>{t("common:loading")}</p>
      ) : blocks.length === 0 ? (
        <p className={panel.message}>{t("admin:authIpBlocksPanel.emptyMessage")}</p>
      ) : (
        blocks.map((block) => (
          <div className={styles.card} key={block.ip_address}>
            <strong>{block.ip_address}</strong>
            <p className={styles.hint}>{block.reason}</p>
            <p className={styles.hint}>
              {t("admin:authIpBlocksPanel.blockedUntilLabel", {
                date: new Date(block.blocked_until).toLocaleString(i18n.language),
              })}
            </p>
            <button
              className={styles.btnGhost}
              disabled={busyIp === block.ip_address}
              onClick={() => unblock(block.ip_address)}
            >
              {t("admin:authIpBlocksPanel.unblockButton")}
            </button>
          </div>
        ))
      )}
    </div>
  );
}
