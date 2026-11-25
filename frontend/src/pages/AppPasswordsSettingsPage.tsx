import { FormEvent, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, AppPasswordRow, CreateAppPasswordResponse, getErrorMessage } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useToast } from "../contexts/ToastContext";
import { formatDate } from "../lib/format";
import panel from "../components/common/Panel.module.css";
import styles from "./AppTokensSettings.module.css";

/**
 * Bluesky クライアント（AT Protocol）用アプリパスワードの一覧・発行・無効化。
 * メインパスワードの代わりにサードパーティの Bluesky クライアントへ渡すためのもの。
 */
export default function AppPasswordsSettingsPage() {
  const { t } = useTranslation();
  const { showError } = useToast();

  const [passwords, setPasswords] = useState<AppPasswordRow[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [revokingId, setRevokingId] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [creating, setCreating] = useState(false);
  // 発行直後のパスワードは一度きりしか表示できない（DBにはハッシュしか残らない）ため、
  // 一覧とは別に保持して専用の表示エリアに出す。
  const [issued, setIssued] = useState<CreateAppPasswordResponse | null>(null);
  const [copied, setCopied] = useState(false);

  const load = useCallback(() => {
    setLoading(true);
    api.appPasswords
      .list()
      .then(setPasswords)
      .catch((e) => showError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }, [showError]);

  useEffect(() => {
    load();
  }, [load]);

  async function revoke(row: AppPasswordRow) {
    if (!window.confirm(t("account:appPasswordsSettings.revokeConfirm", { name: row.name }))) {
      return;
    }
    setRevokingId(row.id);
    try {
      await api.appPasswords.revoke(row.id);
      setPasswords((prev) => prev?.filter((p) => p.id !== row.id) ?? null);
      if (issued?.id === row.id) setIssued(null);
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setRevokingId(null);
    }
  }

  async function create(e: FormEvent) {
    e.preventDefault();
    const name = newName.trim();
    if (!name) return;
    setCreating(true);
    try {
      const result = await api.appPasswords.create(name);
      setIssued(result);
      setCopied(false);
      setNewName("");
      setPasswords((prev) => [
        { id: result.id, name: result.name, created_at: result.created_at },
        ...(prev ?? []),
      ]);
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setCreating(false);
    }
  }

  async function copyIssued() {
    if (!issued) return;
    await navigator.clipboard.writeText(issued.password);
    setCopied(true);
  }

  const center = (
    <>
      <p className={styles.description}>{t("account:appPasswordsSettings.description")}</p>

      {issued && (
        <div className={styles.issuedBox}>
          <p className={styles.issuedWarning}>
            {t("account:appPasswordsSettings.issuedWarning", { name: issued.name })}
          </p>
          <div className={styles.issuedTokenRow}>
            <code className={styles.issuedToken}>{issued.password}</code>
            <button type="button" className={styles.copyBtn} onClick={copyIssued}>
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
          placeholder={t("account:appPasswordsSettings.nameLabel")}
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          maxLength={100}
          required
        />
        <button
          type="submit"
          className={styles.createBtn}
          disabled={creating || !newName.trim()}
        >
          {creating
            ? t("account:appTokensSettings.creatingButton")
            : t("account:appTokensSettings.createButton")}
        </button>
      </form>

      {loading && <p className={panel.message}>{t("common:loading")}</p>}
      {!loading && passwords && passwords.length === 0 && (
        <p className={panel.message}>{t("account:appPasswordsSettings.empty")}</p>
      )}

      {!loading && passwords && passwords.length > 0 && (
        <ul className={styles.list}>
          {passwords.map((row) => (
            <li key={row.id} className={styles.row}>
              <div className={styles.info}>
                <span className={styles.clientName}>{row.name}</span>
                <span className={styles.createdAt}>
                  {t("account:appTokensSettings.issuedAt", { date: formatDate(row.created_at) })}
                </span>
              </div>
              <button
                type="button"
                className={styles.revokeBtn}
                disabled={revokingId === row.id}
                onClick={() => revoke(row)}
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
