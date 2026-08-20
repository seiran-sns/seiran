import { FormEvent, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, AppTokenRow, CreateAppTokenResponse, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import { useToast } from "../contexts/ToastContext";
import { formatDate } from "../lib/format";
import panel from "../components/common/Panel.module.css";
import styles from "./AppTokensSettings.module.css";

/** メインメニュー「設定」内の発行済みアプリトークン一覧・発行・無効化（#60）。 */
export default function AppTokensSettingsPage() {
  const { t } = useTranslation();
  const { showError } = useToast();
  const goBack = useGoBack();

  const [tokens, setTokens] = useState<AppTokenRow[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [revokingId, setRevokingId] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);
  // 発行直後のトークンは一度きりしか表示できない（DBには検証用のjtiしか残らない）ため、
  // 一覧とは別に保持して専用の表示エリアに出す。
  const [issued, setIssued] = useState<CreateAppTokenResponse | null>(null);
  const [copied, setCopied] = useState(false);

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

  async function create(e: FormEvent) {
    e.preventDefault();
    setCreating(true);
    try {
      const result = await api.appTokens.create(newName.trim() || undefined);
      setIssued(result);
      setCopied(false);
      setNewName("");
      setTokens((prev) => [
        { id: result.id, client_name: result.client_name, created_at: result.created_at },
        ...(prev ?? []),
      ]);
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setCreating(false);
    }
  }

  async function copyIssuedToken() {
    if (!issued) return;
    await navigator.clipboard.writeText(issued.token);
    setCopied(true);
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("account:menu.appTokens")}</span>
      </header>

      {issued && (
        <div className={styles.issuedBox}>
          <p className={styles.issuedWarning}>
            {t("account:appTokensSettings.issuedWarning")}
          </p>
          <div className={styles.issuedTokenRow}>
            <code className={styles.issuedToken}>{issued.token}</code>
            <button type="button" className={styles.copyBtn} onClick={copyIssuedToken}>
              {copied
                ? t("account:appTokensSettings.copiedButton")
                : t("account:appTokensSettings.copyButton")}
            </button>
          </div>
          <button type="button" className={styles.dismissBtn} onClick={() => setIssued(null)}>
            {t("account:appTokensSettings.dismissButton")}
          </button>
        </div>
      )}

      <form className={styles.createForm} onSubmit={create}>
        <input
          type="text"
          className={styles.createInput}
          placeholder={t("account:appTokensSettings.nameLabel")}
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          maxLength={100}
        />
        <button type="submit" className={styles.createBtn} disabled={creating}>
          {creating
            ? t("account:appTokensSettings.creatingButton")
            : t("account:appTokensSettings.createButton")}
        </button>
      </form>

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
