import { FormEvent, useEffect, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

type Phase =
  | { kind: "verifying" }
  | { kind: "form"; token: string }
  | { kind: "invalid"; message: string }
  | { kind: "done" };

export default function ResetPassword() {
  const { t } = useTranslation();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();

  const [phase, setPhase] = useState<Phase>({ kind: "verifying" });
  const [password, setPassword] = useState("");
  const [passwordConfirm, setPasswordConfirm] = useState("");
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  // マウント時にトークンを検証する
  useEffect(() => {
    const token = searchParams.get("token");
    if (!token) {
      setPhase({ kind: "invalid", message: t("auth:resetPassword.invalidUrl") });
      return;
    }
    const controller = new AbortController();
    api.auth.verifyResetToken(token, controller.signal)
      .then(() => setPhase({ kind: "form", token }))
      .catch((err) => {
        if (controller.signal.aborted) return;
        setPhase({ kind: "invalid", message: getErrorMessage(err) });
      });
    return () => controller.abort();
  }, [searchParams, t]);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    if (phase.kind !== "form") return;
    setError("");

    if (password.length < 8) {
      setError(t("auth:resetPassword.passwordTooShort"));
      return;
    }
    if (password !== passwordConfirm) {
      setError(t("auth:resetPassword.passwordMismatch"));
      return;
    }

    setSubmitting(true);
    try {
      await api.auth.resetPassword(phase.token, password);
      navigate("/login");
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  if (phase.kind === "verifying") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>{t("common:appName")}</h1>
          <p style={{ textAlign: "center", color: "#a0aec0" }}>{t("auth:resetPassword.verifying")}</p>
        </div>
      </div>
    );
  }

  if (phase.kind === "invalid") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>{t("common:appName")}</h1>
          <h2 className={styles.subtitle}>{t("auth:resetPassword.invalidLinkTitle")}</h2>
          <p className={styles.error} style={{ textAlign: "center" }}>
            {phase.message || t("auth:resetPassword.invalidLinkFallback")}
          </p>
          <p className={styles.link} style={{ marginTop: "1rem" }}>
            <Link to="/forgot-password">{t("auth:resetPassword.retryLink")}</Link>
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:resetPassword.title")}</h2>
        <form onSubmit={handleSubmit} className={styles.form}>
          <label className={styles.label}>
            {t("auth:resetPassword.newPasswordLabel")}
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className={styles.input}
              required
              minLength={8}
              autoFocus
            />
          </label>
          <label className={styles.label}>
            {t("auth:resetPassword.confirmPasswordLabel")}
            <input
              type="password"
              value={passwordConfirm}
              onChange={(e) => setPasswordConfirm(e.target.value)}
              className={styles.input}
              required
              minLength={8}
            />
          </label>
          {error && <p className={styles.error}>{error}</p>}
          <button type="submit" className={styles.button} disabled={submitting}>
            {submitting ? t("auth:resetPassword.updating") : t("auth:resetPassword.submit")}
          </button>
        </form>
        <p className={styles.link}>
          <Link to="/login">{t("auth:resetPassword.backToLoginLink")}</Link>
        </p>
      </div>
    </div>
  );
}
