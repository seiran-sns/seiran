import { FormEvent, useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, ApiError } from "../api/client";
import styles from "./Auth.module.css";

const STORAGE_KEY = "seiran_migration_request";

export interface StoredMigrationRequest {
  id: number;
  token: string;
}

export function loadStoredMigrationRequest(): StoredMigrationRequest | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as StoredMigrationRequest;
    if (typeof parsed.id === "number" && typeof parsed.token === "string") return parsed;
    return null;
  } catch {
    return null;
  }
}

export function storeMigrationRequest(req: StoredMigrationRequest) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(req));
}

export function clearStoredMigrationRequest() {
  localStorage.removeItem(STORAGE_KEY);
}

/**
 * 既存DID転入フロー（`docs/account_migration.md`）の入り口。Bluesky等の既存アカウントの
 * ハンドル・パスワードと、seiran側で新規に名乗るユーザー名・パスワードを入力する。
 * `POST /api/migration/start`が`AUTH_FACTOR_TOKEN_REQUIRED`を返した場合は、
 * 移行元PDSのメール2FAコード入力欄を追加表示して同フォームで再試行する。
 * 成功したら`request_id`/`request_token`を保存し、状態画面（`MigrationStatusPage`）へ遷移する。
 */
export default function MigrateRegister() {
  const { t } = useTranslation();
  const navigate = useNavigate();

  const [requireEmailVerification, setRequireEmailVerification] = useState<boolean | null>(null);

  const [sourceHandle, setSourceHandle] = useState("");
  const [sourcePassword, setSourcePassword] = useState("");
  const [newUsername, setNewUsername] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [email, setEmail] = useState("");

  const [needsAuthFactorToken, setNeedsAuthFactorToken] = useState(false);
  const [authFactorToken, setAuthFactorToken] = useState("");

  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    api.meta().then((meta) => {
      setRequireEmailVerification(meta.requireEmailVerification ?? false);
    }).catch(() => setRequireEmailVerification(false));
  }, []);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      const res = await api.migration.start({
        source_handle: sourceHandle,
        source_password: sourcePassword,
        new_username: newUsername,
        new_password: newPassword,
        auth_factor_token: needsAuthFactorToken ? authFactorToken : undefined,
        email: requireEmailVerification === false ? email : undefined,
      });
      storeMigrationRequest({ id: res.request_id, token: res.request_token });
      navigate("/register/migrate/status");
    } catch (err) {
      if (err instanceof ApiError && err.code === "AUTH_FACTOR_TOKEN_REQUIRED") {
        setNeedsAuthFactorToken(true);
        setError(t("auth:migrateRegister.authFactorTokenRequired"));
      } else {
        setError(getErrorMessage(err));
      }
    } finally {
      setLoading(false);
    }
  }

  if (requireEmailVerification === null) return null;

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:migrateRegister.title")}</h2>
        <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem", fontSize: "0.9rem" }}>
          {t("auth:migrateRegister.description")}
        </p>
        <form onSubmit={handleSubmit} className={styles.form}>
          <label className={styles.label}>
            {t("auth:migrateRegister.sourceHandleLabel")}
            <input
              type="text"
              value={sourceHandle}
              onChange={(e) => setSourceHandle(e.target.value)}
              className={styles.input}
              placeholder="example.bsky.social"
              required
              autoFocus
              disabled={needsAuthFactorToken}
            />
          </label>
          <label className={styles.label}>
            {t("auth:migrateRegister.sourcePasswordLabel")}
            <input
              type="password"
              value={sourcePassword}
              onChange={(e) => setSourcePassword(e.target.value)}
              className={styles.input}
              required
              disabled={needsAuthFactorToken}
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
                required
                autoFocus
              />
            </label>
          )}
          <label className={styles.label}>
            {t("auth:migrateRegister.newUsernameLabel")}
            <input
              type="text"
              value={newUsername}
              onChange={(e) => setNewUsername(e.target.value)}
              className={styles.input}
              required
            />
          </label>
          <label className={styles.label}>
            {t("auth:migrateRegister.newPasswordLabel")}
            <input
              type="password"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
              className={styles.input}
              required
              minLength={8}
            />
          </label>
          {requireEmailVerification === false && (
            <label className={styles.label}>
              {t("auth:register.emailLabel")}
              <input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                className={styles.input}
                required
              />
            </label>
          )}
          {error && <p className={styles.error}>{error}</p>}
          <button type="submit" className={styles.button} disabled={loading}>
            {loading ? t("auth:migrateRegister.submitting") : t("auth:migrateRegister.submit")}
          </button>
        </form>
        <p className={styles.link}>
          <Link to="/register">{t("auth:migrateRegister.backToNormalRegisterLink")}</Link>
        </p>
      </div>
    </div>
  );
}
