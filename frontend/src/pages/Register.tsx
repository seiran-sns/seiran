import { FormEvent, useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

export default function Register() {
  const { t } = useTranslation();
  const navigate = useNavigate();

  // requireEmailVerification フラグ（null = まだロード中）
  const [requireEmailVerification, setRequireEmailVerification] = useState<boolean | null>(null);

  // メール確認フロー用
  const [email, setEmail] = useState("");
  const [sent, setSent] = useState(false);

  // 直接登録フロー用
  const [directEmail, setDirectEmail] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");

  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    api.meta().then((meta) => {
      setRequireEmailVerification(meta.requireEmailVerification ?? false);
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
      await api.auth.requestEmailVerification(email);
      setSent(true);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setLoading(false);
    }
  }

  if (requireEmailVerification === true && sent) {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>{t("common:appName")}</h1>
          <h2 className={styles.subtitle}>{t("auth:register.emailSentTitle")}</h2>
          <p style={{ textAlign: "center", color: "#a0aec0", lineHeight: 1.6 }}>
            {t("auth:register.emailSentDescription", { email })}
          </p>
          <p className={styles.link} style={{ marginTop: "1.5rem" }}>
            <Link to="/login">{t("auth:register.goToLoginLink")}</Link>
          </p>
        </div>
      </div>
    );
  }

  if (requireEmailVerification === true) {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>{t("common:appName")}</h1>
          <h2 className={styles.subtitle}>{t("auth:register.title")}</h2>
          <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem", fontSize: "0.9rem" }}>
            {t("auth:register.verifyDescription")}
          </p>
          <form onSubmit={handleVerifySubmit} className={styles.form}>
            <label className={styles.label}>
              {t("auth:register.emailLabel")}
              <input
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                className={styles.input}
                required
                autoFocus
              />
            </label>
            {error && <p className={styles.error}>{error}</p>}
            <button type="submit" className={styles.button} disabled={loading}>
              {loading ? t("auth:register.sending") : t("auth:register.sendVerificationEmail")}
            </button>
          </form>
          <p className={styles.link}>
            {t("auth:register.alreadyHaveAccountPrefix")} <Link to="/login">{t("auth:register.loginLink")}</Link>
          </p>
        </div>
      </div>
    );
  }

  // ─── 直接登録フロー（requireEmailVerification === false） ─────────

  async function handleDirectSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      const res = await api.auth.registerDirect(directEmail, username, password);
      localStorage.setItem("seiran_token", res.token);
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
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:register.title")}</h2>
        <form onSubmit={handleDirectSubmit} className={styles.form}>
          <label className={styles.label}>
            {t("auth:register.emailLabel")}
            <input
              type="email"
              value={directEmail}
              onChange={(e) => setDirectEmail(e.target.value)}
              className={styles.input}
              required
              autoFocus
            />
          </label>
          <label className={styles.label}>
            {t("auth:register.usernameLabel")}
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              className={styles.input}
              required
            />
          </label>
          <label className={styles.label}>
            {t("auth:register.passwordLabel")}
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className={styles.input}
              required
            />
          </label>
          {error && <p className={styles.error}>{error}</p>}
          <button type="submit" className={styles.button} disabled={loading}>
            {loading ? t("auth:register.submitting") : t("auth:register.submit")}
          </button>
        </form>
        <p className={styles.link}>
          {t("auth:register.alreadyHaveAccountPrefix")} <Link to="/login">{t("auth:register.loginLink")}</Link>
        </p>
      </div>
    </div>
  );
}
