import { FormEvent, useEffect, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import styles from "./Auth.module.css";

type State =
  | { phase: "verifying" }
  | { phase: "form"; registrationToken: string }
  | { phase: "error"; message: string };

export default function VerifyEmail() {
  const { t } = useTranslation();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const { login } = useAuth();

  const [state, setState] = useState<State>({ phase: "verifying" });
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [formError, setFormError] = useState("");
  const [submitting, setSubmitting] = useState(false);

  // マウント時にトークンを検証する
  useEffect(() => {
    const token = searchParams.get("token");
    if (!token) {
      setState({ phase: "error", message: t("auth:verifyEmail.invalidUrl") });
      return;
    }
    const controller = new AbortController();
    api.auth.verifyEmailToken(token, controller.signal)
      .then((res) => setState({ phase: "form", registrationToken: res.registration_token }))
      .catch((err) => {
        if (controller.signal.aborted) return;
        setState({ phase: "error", message: getErrorMessage(err) });
      });
    return () => controller.abort();
  }, [searchParams, t]);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    if (state.phase !== "form") return;
    setFormError("");
    if (password.length < 8) {
      setFormError(t("auth:verifyEmail.passwordTooShort"));
      return;
    }
    setSubmitting(true);
    try {
      const res = await api.auth.register(username, password, state.registrationToken);
      login(res.token, res.user);
      navigate("/");
    } catch (err) {
      setFormError(getErrorMessage(err));
    } finally {
      setSubmitting(false);
    }
  }

  if (state.phase === "verifying") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>{t("common:appName")}</h1>
          <p style={{ textAlign: "center", color: "#a0aec0" }}>{t("auth:verifyEmail.verifying")}</p>
        </div>
      </div>
    );
  }

  if (state.phase === "error") {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>{t("common:appName")}</h1>
          <h2 className={styles.subtitle}>{t("auth:verifyEmail.failedTitle")}</h2>
          <p className={styles.error} style={{ textAlign: "center" }}>{state.message}</p>
          <p className={styles.link} style={{ marginTop: "1rem" }}>
            <Link to="/register">{t("auth:verifyEmail.startOverLink")}</Link>
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:verifyEmail.title")}</h2>
        <p style={{ textAlign: "center", color: "#a0aec0", marginBottom: "1rem", fontSize: "0.9rem" }}>
          {t("auth:verifyEmail.description")}
        </p>
        <form onSubmit={handleSubmit} className={styles.form}>
          <label className={styles.label}>
            {t("auth:verifyEmail.usernameLabel")}
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              className={styles.input}
              required
              autoFocus
              pattern="[a-zA-Z0-9_]+"
              title={t("auth:verifyEmail.usernamePatternTitle")}
            />
          </label>
          <label className={styles.label}>
            {t("auth:verifyEmail.passwordLabel")}
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className={styles.input}
              required
              minLength={8}
            />
          </label>
          {formError && <p className={styles.error}>{formError}</p>}
          <button type="submit" className={styles.button} disabled={submitting}>
            {submitting ? t("auth:verifyEmail.submitting") : t("auth:verifyEmail.submit")}
          </button>
        </form>
      </div>
    </div>
  );
}
