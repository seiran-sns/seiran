import { FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import type { MigrationStatusResponse } from "../api/migration";
import { useAuth } from "../contexts/AuthContext";
import { clearStoredMigrationRequest, loadStoredMigrationRequest } from "./MigrateRegister";
import styles from "./Auth.module.css";

const POLL_INTERVAL_MS = 3000;

const STATUS_LABEL_KEYS: Record<string, string> = {
  fetching_repo: "auth:migrationStatus.step.fetchingRepo",
  awaiting_seiran_email: "auth:migrationStatus.step.awaitingSeiranEmail",
  requesting_plc_signature: "auth:migrationStatus.step.requestingPlcSignature",
  awaiting_plc_token: "auth:migrationStatus.step.awaitingPlcToken",
  submitting_plc: "auth:migrationStatus.step.submittingPlc",
  importing_data: "auth:migrationStatus.step.importingData",
  deactivating_source: "auth:migrationStatus.step.deactivatingSource",
  completed: "auth:migrationStatus.step.completed",
  failed: "auth:migrationStatus.step.failed",
  failed_post_submit: "auth:migrationStatus.step.failedPostSubmit",
  abandoned: "auth:migrationStatus.step.abandoned",
};

/**
 * 既存DID転入フロー（`docs/account_migration.md`）の汎用状態画面。
 * 「現在ステップXを待っています」＋（必要な時のみ）入力欄1個＋リトライボタン、という
 * 単一の画面でほぼ全ステータスを表現する（プランの設計方針そのまま）。
 * `plc_submitted_at`が未設定（`can_abandon`）の間だけ「別DIDで再開／新規DID切替」を出す。
 */
export default function MigrationStatusPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { login } = useAuth();

  const stored = loadStoredMigrationRequest();

  const [statusData, setStatusData] = useState<MigrationStatusResponse | null>(null);
  const [error, setError] = useState("");
  const [inputValue, setInputValue] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const pollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchStatus = useCallback(async () => {
    if (!stored) return;
    try {
      const res = await api.migration.status(stored.id, stored.token);
      setStatusData(res);
      setError("");
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }, [stored]);

  useEffect(() => {
    if (!stored) {
      navigate("/register/migrate", { replace: true });
      return;
    }
    fetchStatus();
    return () => {
      if (pollTimer.current) clearTimeout(pollTimer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ジョブが裏で動くステータスの間だけポーリングする。入力待ち・終端状態では止める。
  useEffect(() => {
    if (!statusData) return;
    const isPolling =
      statusData.retryable &&
      !statusData.needs_input &&
      !["completed", "abandoned"].includes(statusData.status);
    if (!isPolling) return;
    pollTimer.current = setTimeout(fetchStatus, POLL_INTERVAL_MS);
    return () => {
      if (pollTimer.current) clearTimeout(pollTimer.current);
    };
  }, [statusData, fetchStatus]);

  if (!stored) return null;

  async function handleRetry() {
    setError("");
    setSubmitting(true);
    try {
      await api.migration.retry(stored!.id, stored!.token);
      await fetchStatus();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleAbandon() {
    setError("");
    setSubmitting(true);
    try {
      await api.migration.abandon(stored!.id, stored!.token);
      clearStoredMigrationRequest();
      navigate("/register/migrate", { replace: true });
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  function handleStartOver() {
    clearStoredMigrationRequest();
    navigate("/register", { replace: true });
  }

  async function handleInputSubmit(e: FormEvent) {
    e.preventDefault();
    if (!statusData?.needs_input) return;
    setError("");
    setSubmitting(true);
    try {
      if (statusData.needs_input === "seiran_email_token") {
        await api.migration.confirmSeiranEmail(stored!.id, stored!.token, inputValue);
        setInputValue("");
        await fetchStatus();
      } else if (statusData.needs_input === "plc_token") {
        const res = await api.migration.submitPlcToken(stored!.id, stored!.token, inputValue);
        clearStoredMigrationRequest();
        login(res.token, res.user);
        navigate("/", { replace: true });
      }
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  const stepLabelKey = statusData ? STATUS_LABEL_KEYS[statusData.status] : undefined;
  const stepLabel = stepLabelKey ? t(stepLabelKey) : statusData?.status ?? "";

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:migrationStatus.title")}</h2>

        <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem" }}>{stepLabel}</p>

        {statusData?.last_error && <p className={styles.error}>{statusData.last_error}</p>}
        {error && <p className={styles.error}>{error}</p>}

        {statusData?.needs_input && (
          <form onSubmit={handleInputSubmit} className={styles.form}>
            <label className={styles.label}>
              {statusData.needs_input === "seiran_email_token"
                ? t("auth:migrationStatus.seiranEmailTokenLabel")
                : t("auth:migrationStatus.plcTokenLabel")}
              <input
                type="text"
                value={inputValue}
                onChange={(e) => setInputValue(e.target.value)}
                className={styles.input}
                required
                autoFocus
              />
            </label>
            <button type="submit" className={styles.button} disabled={submitting}>
              {t("auth:migrationStatus.submitInput")}
            </button>
          </form>
        )}

        {!statusData?.needs_input && statusData?.retryable && (
          <button type="button" className={styles.button} onClick={handleRetry} disabled={submitting}>
            {t("auth:migrationStatus.retry")}
          </button>
        )}

        {statusData?.can_abandon && (
          <div style={{ marginTop: "1.5rem", textAlign: "center" }}>
            <p style={{ color: "#a0aec0", fontSize: "0.85rem", marginBottom: "0.5rem" }}>
              {t("auth:migrationStatus.abandonDescription")}
            </p>
            <button
              type="button"
              className={styles.button}
              onClick={handleAbandon}
              disabled={submitting}
              style={{ marginBottom: "0.5rem" }}
            >
              {t("auth:migrationStatus.retryWithDifferentAccount")}
            </button>
            <button type="button" className={styles.button} onClick={handleStartOver} disabled={submitting}>
              {t("auth:migrationStatus.startFreshInstead")}
            </button>
          </div>
        )}

        {(statusData?.status === "abandoned" ||
          statusData?.status === "failed" ||
          statusData?.status === "failed_post_submit") && (
          <p className={styles.link} style={{ marginTop: "1rem" }}>
            <button type="button" className={styles.button} onClick={handleStartOver}>
              {t("auth:migrationStatus.backToRegister")}
            </button>
          </p>
        )}
      </div>
    </div>
  );
}
