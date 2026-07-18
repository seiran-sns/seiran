import { FormEvent, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import styles from "./Auth.module.css";

interface Props {
  onComplete: () => void;
}

export default function Setup({ onComplete }: Props) {
  const { t } = useTranslation();
  const { login } = useAuth();
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      const res = await api.setup.initialize(username, email, password);
      login(res.token, res.user);
      onComplete();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("setup:setup.title")}</h2>
        <p className={styles.description}>
          {t("setup:setup.description")}
        </p>
        <form onSubmit={handleSubmit} className={styles.form}>
          <label className={styles.label}>
            {t("setup:setup.usernameLabel")}
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              className={styles.input}
              required
              autoFocus
              autoComplete="username"
            />
          </label>
          <label className={styles.label}>
            {t("setup:setup.emailLabel")}
            <input
              type="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              className={styles.input}
              required
              autoComplete="email"
            />
          </label>
          <label className={styles.label}>
            {t("setup:setup.passwordLabel")}
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className={styles.input}
              required
              minLength={8}
              autoComplete="new-password"
            />
          </label>
          {error && <p className={styles.error}>{error}</p>}
          <button type="submit" className={styles.button} disabled={loading}>
            {loading ? t("setup:setup.submitting") : t("setup:setup.submit")}
          </button>
        </form>
      </div>
    </div>
  );
}
