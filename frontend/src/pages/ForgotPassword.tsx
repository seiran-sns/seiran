import { FormEvent, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

type Phase = "form" | "sent";

export default function ForgotPassword() {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<Phase>("form");
  const [email, setEmail] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      await api.auth.requestPasswordReset(email);
      setPhase("sent");
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setLoading(false);
    }
  }

  if (phase === "sent") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>{t("common:appName")}</h1>
          <h2 className={styles.subtitle}>{t("auth:forgotPassword.sentTitle")}</h2>
          <p style={{ textAlign: "center", color: "#a0aec0", fontSize: "0.9rem", margin: "0 0 24px" }}>
            {t("auth:forgotPassword.sentDescription")}
          </p>
          <p className={styles.link}>
            <Link to="/login">{t("auth:forgotPassword.backToLoginLink")}</Link>
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:forgotPassword.title")}</h2>
        <p className={styles.description}>
          {t("auth:forgotPassword.description")}
        </p>
        <form onSubmit={handleSubmit} className={styles.form}>
          <label className={styles.label}>
            {t("auth:forgotPassword.emailLabel")}
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
            {loading ? t("auth:forgotPassword.sending") : t("auth:forgotPassword.submit")}
          </button>
        </form>
        <p className={styles.link}>
          <Link to="/login">{t("auth:forgotPassword.backToLoginLink")}</Link>
        </p>
      </div>
    </div>
  );
}
