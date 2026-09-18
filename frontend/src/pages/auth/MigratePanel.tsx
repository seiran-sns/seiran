import { FormEvent, useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, ApiError } from "../../api/client";
import { storeMigrationRequest } from "../migrationStorage";
import styles from "../Auth.module.css";

/** 他パネルへ切り替えて戻ってきても再開できるよう、親（`AuthCarouselPage`）に持たせる状態。 */
export interface MigratePanelState {
  sourceHandle: string;
  sourcePassword: string;
  newUsername: string;
  newPassword: string;
  email: string;
  needsAuthFactorToken: boolean;
  authFactorToken: string;
}

export const MIGRATE_PANEL_INITIAL_STATE: MigratePanelState = {
  sourceHandle: "",
  sourcePassword: "",
  newUsername: "",
  newPassword: "",
  email: "",
  needsAuthFactorToken: false,
  authFactorToken: "",
};

interface MigratePanelProps {
  state: MigratePanelState;
  onChange: (patch: Partial<MigratePanelState>) => void;
}

/**
 * ログインカルーセル（issue #243）の「Blueskyから転入」パネル本体。既存DID転入フロー
 * （`docs/account_migration.md`）の入り口。外枠（見出し・カード）は親の`AuthCarouselPage`が
 * 持つため、フォームのみを描画する。`POST /api/migration/start`が
 * `AUTH_FACTOR_TOKEN_REQUIRED`を返した場合は、移行元PDSのメール2FAコード入力欄を追加表示して
 * 同フォームで再試行する。成功したら`request_id`/`request_token`を保存し、状態画面
 * （`MigrationStatusPage`）へ遷移する。
 *
 * このパネルは非アクティブ時に実際にアンマウントされる（`RegisterPanel`と同じ理由、#243）。
 * 入力途中の値・2FAコード入力段階への遷移は`state`/`onChange`経由で親に持たせ、他パネルへ
 * 切り替えて戻ってきても続きから再開できるようにしている。
 */
export default function MigratePanel({ state, onChange }: MigratePanelProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();

  const [requireEmailVerification, setRequireEmailVerification] = useState<boolean | null>(null);
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
        source_handle: state.sourceHandle,
        source_password: state.sourcePassword,
        new_username: state.newUsername,
        new_password: state.newPassword,
        auth_factor_token: state.needsAuthFactorToken ? state.authFactorToken : undefined,
        email: requireEmailVerification === false ? state.email : undefined,
      });
      storeMigrationRequest({ id: res.request_id, token: res.request_token });
      navigate("/register/migrate/status");
    } catch (err) {
      if (err instanceof ApiError && err.code === "AUTH_FACTOR_TOKEN_REQUIRED") {
        onChange({ needsAuthFactorToken: true });
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
        {requireEmailVerification === false && (
          <label className={styles.label}>
            {t("auth:register.emailLabel")}
            <input
              type="email"
              value={state.email}
              onChange={(e) => onChange({ email: e.target.value })}
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
    </>
  );
}
