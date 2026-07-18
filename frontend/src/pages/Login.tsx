import { FormEvent, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import styles from "./Auth.module.css";

export default function Login() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const { login } = useAuth();
  const [identifier, setIdentifier] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      const res = await api.auth.login(identifier, password);
      login(res.token, res.user);
      const redirectTo = searchParams.get("redirect");
      navigate(redirectTo && redirectTo.startsWith("/") ? redirectTo : "/");
    } catch (err) {
      setError(getErrorMessage(err) || t("auth:login.genericError"));
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:login.title")}</h2>
        <form onSubmit={handleSubmit} className={styles.form}>
          <label className={styles.label}>
            {t("auth:login.identifierLabel")}
            <input
              type="text"
              value={identifier}
              onChange={(e) => setIdentifier(e.target.value)}
              className={styles.input}
              required
              autoFocus
            />
          </label>
          <label className={styles.label}>
            {t("auth:login.passwordLabel")}
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
            {loading ? t("auth:login.submitting") : t("auth:login.submit")}
          </button>
        </form>
        <p className={styles.link}>
          {t("auth:login.forgotPasswordPrefix")} <Link to="/forgot-password">{t("auth:login.forgotPasswordLink")}</Link>
        </p>
        <p className={styles.link}>
          {t("auth:login.noAccountPrefix")} <Link to="/register">{t("auth:login.registerLink")}</Link>
        </p>
      </div>
    </div>
  );
}
