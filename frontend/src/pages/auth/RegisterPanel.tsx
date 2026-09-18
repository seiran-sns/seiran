import { FormEvent, useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../../api/client";
import { useAuth } from "../../contexts/AuthContext";
import Turnstile from "../../components/Turnstile";
import styles from "../Auth.module.css";

/** 他パネルへ切り替えて戻ってきても再開できるよう、親（`AuthCarouselPage`）に持たせる状態。 */
export interface RegisterPanelState {
  email: string;
  sent: boolean;
  directEmail: string;
  username: string;
  password: string;
  birthday: string;
}

export const REGISTER_PANEL_INITIAL_STATE: RegisterPanelState = {
  email: "",
  sent: false,
  directEmail: "",
  username: "",
  password: "",
  birthday: "",
};

interface RegisterPanelProps {
  state: RegisterPanelState;
  onChange: (patch: Partial<RegisterPanelState>) => void;
}

/**
 * ログインカルーセル（issue #243）の「サインアップ」パネル本体。外枠（見出し・カード）は
 * 親の`AuthCarouselPage`が持つため、フォームのみを描画する。
 *
 * このパネルは非アクティブ時に実際にアンマウントされる（同じラベル文言を持つ他パネルの
 * フィールドとDOM上で衝突しないよう、アクティブなパネルだけを描画する設計。#243）。
 * そのためメール確認送信済み等の「途中状態」は`state`/`onChange`経由で親に持たせ、
 * 他パネルへ切り替えて戻ってきても続きから再開できるようにしている。
 */
export default function RegisterPanel({ state, onChange }: RegisterPanelProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { login } = useAuth();

  // requireEmailVerification フラグ（null = まだロード中）。再マウントのたびに再取得するが
  // `/api/meta`は軽量・冪等なので問題ない。
  const [requireEmailVerification, setRequireEmailVerification] = useState<boolean | null>(null);
  const [turnstileSiteKey, setTurnstileSiteKey] = useState("");
  const [turnstileToken, setTurnstileToken] = useState("");

  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    api.meta().then((meta) => {
      setRequireEmailVerification(meta.requireEmailVerification ?? false);
      setTurnstileSiteKey(meta.turnstileSiteKey ?? "");
    }).catch(() => {
      // メタ取得失敗時はデフォルト false
      setRequireEmailVerification(false);
    });
  }, []);

  // ─── メール確認フロー ────────────────────────────────────────────

  async function handleVerifySubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      await api.auth.requestEmailVerification(state.email, turnstileToken);
      onChange({ sent: true });
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setLoading(false);
    }
  }

  if (requireEmailVerification === true && state.sent) {
    return (
      <>
        <p style={{ textAlign: "center", color: "#a0aec0", lineHeight: 1.6 }}>
          {t("auth:register.emailSentDescription", { email: state.email })}
        </p>
        <p className={styles.link} style={{ marginTop: "1.5rem" }}>
          <Link to="/login">{t("auth:register.goToLoginLink")}</Link>
        </p>
      </>
    );
  }

  if (requireEmailVerification === true) {
    return (
      <>
        <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem", fontSize: "0.9rem" }}>
          {t("auth:register.verifyDescription")}
        </p>
        <form onSubmit={handleVerifySubmit} className={styles.form}>
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
          <Turnstile siteKey={turnstileSiteKey} onToken={setTurnstileToken} />
          {error && <p className={styles.error}>{error}</p>}
          <button
            type="submit"
            className={styles.button}
            disabled={loading || (!!turnstileSiteKey && !turnstileToken)}
          >
            {loading ? t("auth:register.sending") : t("auth:register.sendVerificationEmail")}
          </button>
        </form>
        <p className={styles.link}>
          {t("auth:register.alreadyHaveAccountPrefix")} <Link to="/login">{t("auth:register.loginLink")}</Link>
        </p>
        <p className={styles.link}>
          <Link to="/register/migrate">{t("auth:register.migrateInstead")}</Link>
        </p>
      </>
    );
  }

  // ─── 直接登録フロー（requireEmailVerification === false） ─────────

  async function handleDirectSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      const res = await api.auth.registerDirect(
        state.directEmail,
        state.username,
        state.password,
        turnstileToken,
        state.birthday
      );
      login(res.token, res.user);
      navigate("/");
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setLoading(false);
    }
  }

  // ロード中は空を表示
  if (requireEmailVerification === null) {
    return null;
  }

  return (
    <>
      <form onSubmit={handleDirectSubmit} className={styles.form}>
        <label className={styles.label}>
          {t("auth:register.emailLabel")}
          <input
            type="email"
            value={state.directEmail}
            onChange={(e) => onChange({ directEmail: e.target.value })}
            className={styles.input}
            required
            autoFocus
          />
        </label>
        <label className={styles.label}>
          {t("auth:register.usernameLabel")}
          <input
            type="text"
            value={state.username}
            onChange={(e) => onChange({ username: e.target.value })}
            className={styles.input}
            required
          />
        </label>
        <label className={styles.label}>
          {t("auth:register.passwordLabel")}
          <input
            type="password"
            value={state.password}
            onChange={(e) => onChange({ password: e.target.value })}
            className={styles.input}
            required
          />
        </label>
        <label className={styles.label}>
          {t("auth:register.birthdayLabel")}
          <input
            type="date"
            value={state.birthday}
            onChange={(e) => onChange({ birthday: e.target.value })}
            className={styles.input}
          />
        </label>
        <Turnstile siteKey={turnstileSiteKey} onToken={setTurnstileToken} />
        {error && <p className={styles.error}>{error}</p>}
        <button type="submit" className={styles.button} disabled={loading || (!!turnstileSiteKey && !turnstileToken)}>
          {loading ? t("auth:register.submitting") : t("auth:register.submit")}
        </button>
      </form>
      <p className={styles.link}>
        {t("auth:register.alreadyHaveAccountPrefix")} <Link to="/login">{t("auth:register.loginLink")}</Link>
      </p>
      <p className={styles.link}>
        <Link to="/register/migrate">{t("auth:register.migrateInstead")}</Link>
      </p>
    </>
  );
}
