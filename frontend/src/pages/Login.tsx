import { FormEvent, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, isTotpRequired } from "../api/client";
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

  // #65: TOTP有効化済みユーザーの場合、パスワード検証後にこのstateへ切り替わり
  // 二段階目（コード入力）を表示する。
  const [pendingToken, setPendingToken] = useState<string | null>(null);
  const [totpCode, setTotpCode] = useState("");
  const [totpError, setTotpError] = useState("");
  const [totpLoading, setTotpLoading] = useState(false);
  const [disableEmailSent, setDisableEmailSent] = useState(false);

  function finishLogin(res: { token: string; user: Parameters<typeof login>[1] }) {
    login(res.token, res.user);
    const redirectTo = searchParams.get("redirect");
    navigate(redirectTo && redirectTo.startsWith("/") ? redirectTo : "/");
  }

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError("");
    setLoading(true);
    try {
      const res = await api.auth.login(identifier, password);
      if (isTotpRequired(res)) {
        setPendingToken(res.pending_token);
      } else {
        finishLogin(res);
      }
    } catch (err) {
      setError(getErrorMessage(err) || t("auth:login.genericError"));
    } finally {
      setLoading(false);
    }
  }

  async function handleTotpSubmit(e: FormEvent) {
    e.preventDefault();
    if (!pendingToken) return;
    setTotpError("");
    setTotpLoading(true);
    try {
      const res = await api.auth.totp.verify(pendingToken, totpCode);
      finishLogin(res);
    } catch (err) {
      setTotpError(getErrorMessage(err) || t("auth:totpVerify.invalidCode"));
    } finally {
      setTotpLoading(false);
    }
  }

  async function handleLostAccess() {
    if (!pendingToken) return;
    try {
      await api.auth.totp.requestDisableEmail(pendingToken);
      setDisableEmailSent(true);
    } catch (err) {
      setTotpError(getErrorMessage(err) || t("auth:login.genericError"));
    }
  }

  if (pendingToken) {
    return (
      <div className={styles.container}>
        <div className={styles.card}>
          <h1 className={styles.title}>{t("common:appName")}</h1>
          <h2 className={styles.subtitle}>{t("auth:totpVerify.title")}</h2>
          <p>{t("auth:totpVerify.description")}</p>
          <form onSubmit={handleTotpSubmit} className={styles.form}>
            <label className={styles.label}>
              {t("auth:totpVerify.codeLabel")}
              <input
                type="text"
                value={totpCode}
                onChange={(e) => setTotpCode(e.target.value)}
                className={styles.input}
                placeholder={t("auth:totpVerify.codePlaceholder") ?? undefined}
                autoComplete="one-time-code"
                required
                autoFocus
              />
            </label>
            {totpError && <p className={styles.error}>{totpError}</p>}
            <button type="submit" className={styles.button} disabled={totpLoading}>
              {totpLoading ? t("auth:totpVerify.submitting") : t("auth:totpVerify.submit")}
            </button>
          </form>
          {disableEmailSent ? (
            <p className={styles.link}>{t("auth:totpVerify.disableEmailSent")}</p>
          ) : (
            <p className={styles.link}>
              <button type="button" className={styles.linkButton} onClick={handleLostAccess}>
                {t("auth:totpVerify.lostAccessLink")}
              </button>
            </p>
          )}
          <p className={styles.link}>
            <button type="button" className={styles.linkButton} onClick={() => setPendingToken(null)}>
              {t("auth:totpVerify.backToLoginLink")}
            </button>
          </p>
        </div>
      </div>
    );
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
