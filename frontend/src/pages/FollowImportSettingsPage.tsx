import { useEffect, useRef, useState } from "react";
import type { DragEvent } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import type { FollowImportStatusResponse } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import panel from "../components/common/Panel.module.css";
import styles from "./FollowImportSettings.module.css";

const POLL_INTERVAL_MS = 1500;

/** 設定画面「🚚 インポート・エクスポート」。改行区切りのID一覧（隠し仕様として各行を
 * カンマ区切りで分割し1列目のみを識別子として読む、Misskeyフォローエクスポート対応）を
 * 貼り付け or .txt ドラッグ&ドロップで読み込み、非同期ジョブとして一括フォローする。 */
export default function FollowImportSettingsPage() {
  const { t } = useTranslation();
  const goBack = useGoBack();

  const [text, setText] = useState("");
  const [status, setStatus] = useState<FollowImportStatusResponse | null>(null);
  const [starting, setStarting] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [error, setError] = useState("");
  const [dragActive, setDragActive] = useState(false);
  const pollRef = useRef<number | null>(null);

  function stopPolling() {
    if (pollRef.current !== null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
  }

  function startPolling() {
    stopPolling();
    pollRef.current = window.setInterval(async () => {
      try {
        const s = await api.followImport.status();
        setStatus(s);
        if (s.status !== "running") stopPolling();
      } catch {
        stopPolling();
      }
    }, POLL_INTERVAL_MS);
  }

  useEffect(() => {
    let cancelled = false;
    api.followImport
      .status()
      .then((s) => {
        if (cancelled) return;
        setStatus(s);
        if (s.status === "running") startPolling();
      })
      .catch(() => {
        /* 初回取得失敗は無視（idle相当のまま表示） */
      });
    return () => {
      cancelled = true;
      stopPolling();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function onImport() {
    if (!text.trim() || starting) return;
    setStarting(true);
    setError("");
    try {
      await api.followImport.start(text);
      setText("");
      const s = await api.followImport.status();
      setStatus(s);
      if (s.status === "running") startPolling();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setStarting(false);
    }
  }

  async function onCancel() {
    if (cancelling) return;
    setCancelling(true);
    setError("");
    try {
      await api.followImport.cancel();
      stopPolling();
      const s = await api.followImport.status();
      setStatus(s);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setCancelling(false);
    }
  }

  function readFile(file: File) {
    const reader = new FileReader();
    reader.onload = () => setText(String(reader.result ?? ""));
    reader.onerror = () => setError(t("account:importExport.fileReadError"));
    reader.readAsText(file);
  }

  function onDrop(e: DragEvent<HTMLTextAreaElement>) {
    e.preventDefault();
    setDragActive(false);
    const file = e.dataTransfer.files?.[0];
    if (file) readFile(file);
  }

  const isRunning = status?.status === "running";
  const remaining = status ? Math.max(status.total - status.processed, 0) : 0;

  const statusLabel = (() => {
    switch (status?.status) {
      case "running":
        return t("account:importExport.statusRunning");
      case "completed":
        return t("account:importExport.statusCompleted");
      case "cancelled":
        return t("account:importExport.statusCancelled");
      default:
        return null;
    }
  })();

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("account:importExport.title")}</span>
      </header>

      <div className={styles.section}>
        <h3 className={styles.sectionTitle}>{t("account:importExport.followImportTitle")}</h3>
        <p className={styles.description}>{t("account:importExport.description")}</p>

        {error && <p className={styles.error}>{error}</p>}

        <textarea
          className={dragActive ? `${styles.textarea} ${styles.dragActive}` : styles.textarea}
          value={text}
          placeholder={t("account:importExport.textareaPlaceholder")}
          disabled={isRunning || starting}
          onChange={(e) => setText(e.target.value)}
          onDragOver={(e) => {
            e.preventDefault();
            setDragActive(true);
          }}
          onDragLeave={() => setDragActive(false)}
          onDrop={onDrop}
        />

        <button
          type="button"
          className={styles.importButton}
          disabled={isRunning || starting || !text.trim()}
          onClick={onImport}
        >
          {starting
            ? t("account:importExport.importing")
            : t("account:importExport.importButton")}
        </button>

        {status && status.status !== "idle" && (
          <div className={styles.progress}>
            {statusLabel && <p className={styles.progressStatus}>{statusLabel}</p>}
            {isRunning && (
              <p className={styles.progressLine}>
                {t("account:importExport.remainingLabel", { count: remaining })}
              </p>
            )}
            <p className={styles.progressLine}>
              {t("account:importExport.succeededLabel", { count: status.succeeded })}
              {" / "}
              {t("account:importExport.alreadyFollowingLabel", { count: status.alreadyFollowing })}
              {" / "}
              {t("account:importExport.failedLabel", { count: status.failed })}
            </p>
            {isRunning && (
              <button
                type="button"
                className={styles.cancelButton}
                disabled={cancelling}
                onClick={onCancel}
              >
                {t("account:importExport.cancelButton")}
              </button>
            )}
          </div>
        )}
      </div>
    </>
  );

  return <AppShell center={center} />;
}
