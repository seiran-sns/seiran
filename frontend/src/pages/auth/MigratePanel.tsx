import { FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, ApiError } from "../../api/client";
import { isAuthResponse, type MigrationStatusResponse } from "../../api/migration";
import { useAuth } from "../../contexts/AuthContext";
import TermsOfServiceField from "../../components/auth/TermsOfServiceField";
import RegistrationUnavailableNotice from "../../components/auth/RegistrationUnavailableNotice";
import { useSiteMeta } from "../../contexts/SiteMetaContext";
import {
  clearStoredMigrationRequest,
  loadStoredMigrationRequest,
  storeMigrationRequest,
  StoredMigrationRequest,
} from "../migrationStorage";
import { migrationStepLabel } from "../migrationStatusLabels";
import styles from "../Auth.module.css";

/** アカウント（`users`/`actors`）が作られた後の転入の状態。 */
const ACCOUNT_CREATED_STATUSES = ["importing_data", "importing_blobs", "deactivating_source", "completed"];

/** 他パネルへ切り替えて戻ってきても再開できるよう、親（`AuthCarouselPage`）に持たせる状態。 */
export interface MigratePanelState {
  sourceHandle: string;
  sourcePassword: string;
  newUsername: string;
  newPassword: string;
  needsAuthFactorToken: boolean;
  authFactorToken: string;
  /** 移行元PDSの`createSession`がメールアドレスを返さなかった場合のみ表示・使用する
   * フォールバック欄（Blueskyのapp password認証では`email`が返らない）。 */
  needsEmail: boolean;
  email: string;
  agreeTos: boolean;
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
  agreeTos: false,
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
  const [searchParams, setSearchParams] = useSearchParams();
  const resumeToken = searchParams.get("resume");

  if (resumeToken) {
    return (
      <MigrateResumeView
        token={resumeToken}
        onResumed={(req) => {
          storeMigrationRequest(req);
          setStored(req);
          setSearchParams({}, { replace: true });
        }}
        onDismiss={() => setSearchParams({}, { replace: true })}
      />
    );
  }

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

interface MigrateResumeViewProps {
  token: string;
  onResumed: (req: StoredMigrationRequest) => void;
  onDismiss: () => void;
}

/** 案内メールの再開リンク（`/register/migrate?resume=...`）から来たとき、転入トークンを
 * 発行し直してもらって進行状況の表示へ移る。 */
function MigrateResumeView({ token, onResumed, onDismiss }: MigrateResumeViewProps) {
  const { t } = useTranslation();
  const [error, setError] = useState("");
  const started = useRef(false);

  useEffect(() => {
    // StrictMode の二重実行でトークン（使い捨て）を2回使わないようにする。
    if (started.current) return;
    started.current = true;
    api.migration
      .resume(token)
      .then((res) => onResumed({ id: res.request_id, token: res.request_token }))
      .catch((err) => setError(getErrorMessage(err)));
  }, [token, onResumed]);

  if (!error) {
    return <p style={{ textAlign: "center", color: "#a0aec0" }}>{t("auth:migrationStatus.resuming")}</p>;
  }
  return (
    <>
      <p className={styles.error}>{error}</p>
      <button type="button" className={styles.button} onClick={onDismiss}>
        {t("auth:migrationStatus.backToRegister")}
      </button>
    </>
  );
}

interface MigrateFormViewProps {
  state: MigratePanelState;
  onChange: (patch: Partial<MigratePanelState>) => void;
  onStarted: (req: StoredMigrationRequest) => void;
}

function MigrateFormView({ state, onChange, onStarted }: MigrateFormViewProps) {
  const { t } = useTranslation();
  const { accountLimitReached, registrationClosed } = useSiteMeta();
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
        agree_tos: state.agreeTos,
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

  if (registrationClosed || accountLimitReached) {
    return <RegistrationUnavailableNotice />;
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
          <small style={{ color: "#a0aec0" }}>{t("auth:migrateRegister.sourcePasswordHint")}</small>
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
        <TermsOfServiceField agreed={state.agreeTos} onChange={(agreeTos) => onChange({ agreeTos })} />
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
  const [needsAuthFactorToken, setNeedsAuthFactorToken] = useState(false);
  const [authFactorToken, setAuthFactorToken] = useState("");
  const [submitting, setSubmitting] = useState(false);
  // 別の端末でログイン・再開リンクから戻られ、この端末の転入トークンが無効になった。
  const [superseded, setSuperseded] = useState(false);
  const pollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchStatus = useCallback(async () => {
    try {
      const res = await api.migration.status(stored.id, stored.token);
      setStatusData(res);
      setError("");
    } catch (err) {
      if (err instanceof ApiError && err.code === "MIGRATION_TOKEN_INVALID") {
        setSuperseded(true);
        return;
      }
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

  // この画面で DNS TXT の公開待ちを見ていて、それが裏で明けてアカウントが作られたら、
  // 転入トークンで JWT を受け取ってログイン後の画面へ移る（待ちを見ていない場合に限るのは、
  // ログアウトした人が転入画面を開いただけで再ログインされないようにするため）。
  const sawDnsWait = useRef(false);
  useEffect(() => {
    if (!statusData) return;
    if (statusData.status === "awaiting_dns_txt") sawDnsWait.current = true;
    if (!sawDnsWait.current || !ACCOUNT_CREATED_STATUSES.includes(statusData.status)) return;
    let cancelled = false;
    api.migration
      .session(stored.id, stored.token)
      .then((res) => {
        if (cancelled) return;
        // アカウントができたら転入トークンは要らない（以後の取り込み状況は設定 > インポートが
        // JWT で見る）。
        clearStoredMigrationRequest();
        login(res.token, res.user);
        navigate("/", { replace: true });
      })
      .catch((err) => !cancelled && setError(getErrorMessage(err)));
    return () => {
      cancelled = true;
    };
  }, [statusData, stored, login, navigate]);

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

  async function handleResendPlcToken() {
    setError("");
    setSubmitting(true);
    try {
      await api.migration.resendPlcToken(stored.id, stored.token);
      setInputValue("");
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

  async function handleStartOver() {
    // 打ち切らずに離れると、確保した名前とメールが期限まで残り、同じ名前で新規登録できない。
    if (statusData?.can_abandon) {
      setError("");
      setSubmitting(true);
      try {
        await api.migration.abandon(stored.id, stored.token);
      } catch (err) {
        setError(getErrorMessage(err));
        setSubmitting(false);
        return;
      }
      setSubmitting(false);
    }
    onReset();
    navigate("/register", { replace: true });
  }

  async function handleSourcePasswordSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setSubmitting(true);
    try {
      await api.migration.submitSourcePassword(
        stored.id,
        stored.token,
        inputValue,
        needsAuthFactorToken ? authFactorToken : undefined,
      );
      setInputValue("");
      setAuthFactorToken("");
      setNeedsAuthFactorToken(false);
      await fetchStatus();
    } catch (err) {
      if (err instanceof ApiError && err.code === "AUTH_FACTOR_TOKEN_REQUIRED") {
        setNeedsAuthFactorToken(true);
        setError(t("auth:migrateRegister.authFactorTokenRequired"));
      } else {
        setError(getErrorMessage(err));
      }
    } finally {
      setSubmitting(false);
    }
  }

  async function handleInputSubmit(e: FormEvent) {
    e.preventDefault();
    if (statusData?.needs_input !== "plc_token") return;
    setError("");
    setSubmitting(true);
    try {
      const res = await api.migration.submitPlcToken(stored.id, stored.token, inputValue);
      if (!isAuthResponse(res)) {
        // DNS TXT の公開待ち。ポーリングで待ち、アカウントができたら下の effect がログインする。
        await fetchStatus();
        return;
      }
      // アカウントができたら転入トークンは要らない（取り込みは裏で進み、状況は設定 >
      // インポートが JWT で見る）。
      clearStoredMigrationRequest();
      login(res.token, res.user);
      navigate("/", { replace: true });
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  const stepLabel = migrationStepLabel(t, statusData);

  if (superseded) {
    return (
      <>
        <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem" }}>
          {t("auth:migrationStatus.superseded")}
        </p>
        <button type="button" className={styles.button} onClick={onReset}>
          {t("auth:migrationStatus.backToRegister")}
        </button>
      </>
    );
  }

  return (
    <>
      <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem" }}>{stepLabel}</p>
      {statusData?.expired && <p className={styles.error}>{t("auth:migrationStatus.expired")}</p>}
      {statusData?.expires_at && (
        <p style={{ textAlign: "center", color: "#a0aec0", fontSize: "0.85rem", marginBottom: "1rem" }}>
          {t("auth:migrationStatus.expiresAt", { time: new Date(statusData.expires_at).toLocaleString() })}
        </p>
      )}

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
          <p style={{ color: "#a0aec0", fontSize: "0.85rem", marginTop: "0.5rem" }}>
            {t("auth:migrationStatus.resendPlcTokenDescription")}
          </p>
          <button type="button" className={styles.button} onClick={handleResendPlcToken} disabled={submitting}>
            {t("auth:migrationStatus.resendPlcToken")}
          </button>
        </form>
      )}

      {statusData?.needs_input === "source_password" && (
        <form onSubmit={handleSourcePasswordSubmit} className={styles.form}>
          <p style={{ color: "#a0aec0", fontSize: "0.9rem" }}>
            {t("auth:migrationStatus.sourcePasswordDescription")}
          </p>
          <label className={styles.label}>
            {t("auth:migrateRegister.sourcePasswordLabel")}
            <input
              type="password"
              value={inputValue}
              onChange={(e) => setInputValue(e.target.value)}
              className={styles.input}
              autoComplete="current-password"
              required
              autoFocus
            />
          </label>
          {needsAuthFactorToken && (
            <label className={styles.label}>
              {t("auth:migrateRegister.authFactorTokenLabel")}
              <input
                type="text"
                value={authFactorToken}
                onChange={(e) => setAuthFactorToken(e.target.value)}
                className={styles.input}
                autoComplete="one-time-code"
                required
              />
            </label>
          )}
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
