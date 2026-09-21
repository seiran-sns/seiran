import { FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, ApiError } from "../../api/client";
import type { MigrationStatusResponse } from "../../api/migration";
import { useAuth } from "../../contexts/AuthContext";
import {
  clearStoredMigrationRequest,
  loadStoredMigrationRequest,
  storeMigrationRequest,
  StoredMigrationRequest,
} from "../migrationStorage";
import { migrationStepLabel } from "../migrationStatusLabels";
import styles from "../Auth.module.css";

/** 他パネルへ切り替えて戻ってきても再開できるよう、親（`AuthCarouselPage`）に持たせる状態。 */
export interface MigratePanelState {
  sourceHandle: string;
  sourcePassword: string;
  newUsername: string;
  newPassword: string;
  needsAuthFactorToken: boolean;
  authFactorToken: string;
  /** 移行元PDSの`createSession`がメールアドレスを返さなかった場合のみ表示・使用する
   * フォールバック欄（実機で判明: Blueskyのapp password認証では`email`が返らない）。 */
  needsEmail: boolean;
  email: string;
}

export const MIGRATE_PANEL_INITIAL_STATE: MigratePanelState = {
  sourceHandle: "",
  sourcePassword: "",
  newUsername: "",
  newPassword: "",
  needsAuthFactorToken: false,
  authFactorToken: "",
  needsEmail: false,
  email: "",
};

interface MigratePanelProps {
  state: MigratePanelState;
  onChange: (patch: Partial<MigratePanelState>) => void;
}

const POLL_INTERVAL_MS = 3000;

/**
 * ログインカルーセル（issue #243）の「Blueskyから転入」パネル本体。既存DID転入フロー
 * （`docs/account_migration.md`）の入り口から完了までを、別画面へ遷移せずこのパネル1枚の
 * 中で表示を切り替えながら進める。外枠（見出し・カード）は親の`AuthCarouselPage`が持つため、
 * フォーム・状態表示のみを描画する。
 *
 * 進行中のリクエストがあるかどうかは`migrationStorage`（localStorage）を見て判定する。
 * `state`/`onChange`はこのパネルが非アクティブ時にアンマウントされても入力欄の値を
 * 保持するためのもの（`RegisterPanel`と同じ理由、#243）で、開始前のフォーム入力のみが
 * 対象——開始後の進行状況は`localStorage`＋サーバーへの都度問い合わせで復元できるため
 * ここには含めない。
 */
export default function MigratePanel({ state, onChange }: MigratePanelProps) {
  const [stored, setStored] = useState<StoredMigrationRequest | null>(() => loadStoredMigrationRequest());

  if (stored) {
    return (
      <MigrateStatusView
        stored={stored}
        onReset={() => {
          clearStoredMigrationRequest();
          setStored(null);
        }}
      />
    );
  }

  return (
    <MigrateFormView
      state={state}
      onChange={onChange}
      onStarted={(req) => {
        storeMigrationRequest(req);
        setStored(req);
      }}
    />
  );
}

interface MigrateFormViewProps {
  state: MigratePanelState;
  onChange: (patch: Partial<MigratePanelState>) => void;
  onStarted: (req: StoredMigrationRequest) => void;
}

function MigrateFormView({ state, onChange, onStarted }: MigrateFormViewProps) {
  const { t } = useTranslation();
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      const res = await api.migration.start({
        source_handle: state.sourceHandle,
        source_password: state.sourcePassword,
        new_username: state.newUsername,
        new_password: state.newPassword,
        auth_factor_token: state.needsAuthFactorToken ? state.authFactorToken : undefined,
        email: state.needsEmail ? state.email : undefined,
      });
      onStarted({ id: res.request_id, token: res.request_token });
    } catch (err) {
      if (err instanceof ApiError && err.code === "AUTH_FACTOR_TOKEN_REQUIRED") {
        onChange({ needsAuthFactorToken: true });
        setError(t("auth:migrateRegister.authFactorTokenRequired"));
      } else if (err instanceof ApiError && err.code === "SOURCE_EMAIL_REQUIRED") {
        onChange({ needsEmail: true });
        setError(getErrorMessage(err));
      } else {
        setError(getErrorMessage(err));
      }
    } finally {
      setLoading(false);
    }
  }

  return (
    <>
      <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem", fontSize: "0.9rem" }}>
        {t("auth:migrateRegister.description")}
      </p>
      <form onSubmit={handleSubmit} className={styles.form}>
        <label className={styles.label}>
          {t("auth:migrateRegister.sourceHandleLabel")}
          <input
            type="text"
            value={state.sourceHandle}
            onChange={(e) => onChange({ sourceHandle: e.target.value })}
            className={styles.input}
            placeholder="example.bsky.social"
            required
            autoFocus
            disabled={state.needsAuthFactorToken}
          />
        </label>
        <label className={styles.label}>
          {t("auth:migrateRegister.sourcePasswordLabel")}
          <input
            type="password"
            value={state.sourcePassword}
            onChange={(e) => onChange({ sourcePassword: e.target.value })}
            className={styles.input}
            required
            disabled={state.needsAuthFactorToken}
          />
        </label>
        {state.needsAuthFactorToken && (
          <label className={styles.label}>
            {t("auth:migrateRegister.authFactorTokenLabel")}
            <input
              type="text"
              value={state.authFactorToken}
              onChange={(e) => onChange({ authFactorToken: e.target.value })}
              className={styles.input}
              required
              autoFocus
            />
          </label>
        )}
        <label className={styles.label}>
          {t("auth:migrateRegister.newUsernameLabel")}
          <input
            type="text"
            value={state.newUsername}
            onChange={(e) => onChange({ newUsername: e.target.value })}
            className={styles.input}
            required
          />
        </label>
        <label className={styles.label}>
          {t("auth:migrateRegister.newPasswordLabel")}
          <input
            type="password"
            value={state.newPassword}
            onChange={(e) => onChange({ newPassword: e.target.value })}
            className={styles.input}
            required
            minLength={8}
          />
        </label>
        {state.needsEmail && (
          <label className={styles.label}>
            {t("auth:register.emailLabel")}
            <input
              type="email"
              value={state.email}
              onChange={(e) => onChange({ email: e.target.value })}
              className={styles.input}
              required
              autoFocus
            />
          </label>
        )}
        {error && <p className={styles.error}>{error}</p>}
        <button type="submit" className={styles.button} disabled={loading}>
          {loading ? t("auth:migrateRegister.submitting") : t("auth:migrateRegister.submit")}
        </button>
      </form>
    </>
  );
}

interface MigrateStatusViewProps {
  stored: StoredMigrationRequest;
  onReset: () => void;
}

function MigrateStatusView({ stored, onReset }: MigrateStatusViewProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { login } = useAuth();

  const [statusData, setStatusData] = useState<MigrationStatusResponse | null>(null);
  const [error, setError] = useState("");
  const [inputValue, setInputValue] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const pollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchStatus = useCallback(async () => {
    try {
      const res = await api.migration.status(stored.id, stored.token);
      setStatusData(res);
      setError("");
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }, [stored]);

  useEffect(() => {
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

  async function handleRetry() {
    setError("");
    setSubmitting(true);
    try {
      await api.migration.retry(stored.id, stored.token);
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
      await api.migration.abandon(stored.id, stored.token);
      onReset();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  function handleStartOver() {
    onReset();
    navigate("/register", { replace: true });
  }

  async function handleInputSubmit(e: FormEvent) {
    e.preventDefault();
    if (statusData?.needs_input !== "plc_token") return;
    setError("");
    setSubmitting(true);
    try {
      const res = await api.migration.submitPlcToken(stored.id, stored.token, inputValue);
      // localStorageのmigration_token/idはここではクリアしない——`importing_data`以降は
      // `MigrationImportingPage`（ログイン後専用画面）が同じ`/api/migration/:id/status`を
      // 引き続きポーリングして進捗表示に使うため、完了検知した時点でそちらがクリアする。
      login(res.token, res.user);
      navigate("/", { replace: true });
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  const stepLabel = migrationStepLabel(t, statusData);

  return (
    <>
      <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem" }}>{stepLabel}</p>

      {statusData?.last_error && <p className={styles.error}>{statusData.last_error}</p>}
      {error && <p className={styles.error}>{error}</p>}

      {statusData?.needs_input === "plc_token" && (
        <form onSubmit={handleInputSubmit} className={styles.form}>
          <label className={styles.label}>
            {t("auth:migrationStatus.plcTokenLabel")}
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
    </>
  );
}
