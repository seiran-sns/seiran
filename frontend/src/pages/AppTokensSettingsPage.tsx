import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, AppTokenRow, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import { useToast } from "../contexts/ToastContext";
import { formatDate } from "../lib/format";
import panel from "../components/common/Panel.module.css";
import styles from "./AppTokensSettings.module.css";

/** メインメニュー「設定」内の発行済みアプリトークン一覧・無効化（#60）。 */
export default function AppTokensSettingsPage() {
  const { t } = useTranslation();
  const { showError } = useToast();
  const goBack = useGoBack();

  const [tokens, setTokens] = useState<AppTokenRow[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [revokingId, setRevokingId] = useState<string | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    api.appTokens
      .list()
      .then(setTokens)
      .catch((e) => showError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }, [showError]);

  useEffect(() => {
    load();
  }, [load]);

  async function revoke(token: AppTokenRow) {
    setRevokingId(token.id);
    try {
      await api.appTokens.revoke(token.id);
      setTokens((prev) => prev?.filter((tk) => tk.id !== token.id) ?? null);
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setRevokingId(null);
    }
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("account:menu.appTokens")}</span>
      </header>

      {loading && <p className={panel.message}>{t("common:loading")}</p>}
      {!loading && tokens && tokens.length === 0 && (
        <p className={panel.message}>{t("account:appTokensSettings.empty")}</p>
      )}

      {!loading && tokens && tokens.length > 0 && (
        <ul className={styles.list}>
          {tokens.map((token) => (
            <li key={token.id} className={styles.row}>
              <div className={styles.info}>
                <span className={styles.clientName}>{token.client_name}</span>
                <span className={styles.createdAt}>
                  {t("account:appTokensSettings.issuedAt", { date: formatDate(token.created_at) })}
                </span>
              </div>
              <button
                type="button"
                className={styles.revokeBtn}
                disabled={revokingId === token.id}
                onClick={() => revoke(token)}
              >
                {t("account:appTokensSettings.revokeButton")}
              </button>
            </li>
          ))}
        </ul>
      )}
    </>
  );

  return <AppShell center={center} />;
}
