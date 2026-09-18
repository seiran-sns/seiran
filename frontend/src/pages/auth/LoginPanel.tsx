import { FormEvent, useEffect, useState } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, isTotpRequired } from "../../api/client";
import { useAuth } from "../../contexts/AuthContext";
import Turnstile from "../../components/Turnstile";
import styles from "../Auth.module.css";

/**
 * ログインカルーセル（issue #243）の「ログイン」パネル本体。外枠（見出し・カード）は
 * 親の`AuthCarouselPage`が持つため、フォームのみを描画する。
 *
 * `/forgot-password`も`panelFromPath`上は「ログイン」パネルとして扱われる（専用タブを
 * 持たず、ログインパネルの中身がパスワードリセット申請フォームに差し替わる形。マイケルの
 * 指示）。ログインパネルはこの2ルート間で常にマウントされたままなので、パスワード
 * リセット申請の入力途中状態もこのコンポーネント自身のstateで問題ない。
 */
export default function LoginPanel() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const [searchParams] = useSearchParams();
  const { login } = useAuth();
  const [identifier, setIdentifier] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [turnstileSiteKey, setTurnstileSiteKey] = useState("");
  const [turnstileToken, setTurnstileToken] = useState("");

  useEffect(() => {
    api.meta().then((meta) => setTurnstileSiteKey(meta.turnstileSiteKey ?? "")).catch(() => {});
  }, []);

  // #65: TOTP有効化済みユーザーの場合、パスワード検証後にこのstateへ切り替わり
  // 二段階目（コード入力）を表示する。
  const [pendingToken, setPendingToken] = useState<string | null>(null);
  const [totpCode, setTotpCode] = useState("");
  const [totpError, setTotpError] = useState("");
  const [totpLoading, setTotpLoading] = useState(false);
  const [disableEmailSent, setDisableEmailSent] = useState(false);

  // パスワードリセット申請（旧`/forgot-password`ページ）
  const [fpEmail, setFpEmail] = useState("");
  const [fpSent, setFpSent] = useState(false);
  const [fpError, setFpError] = useState("");
  const [fpLoading, setFpLoading] = useState(false);

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
      const res = await api.auth.login(identifier, password, turnstileToken);
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

  async function handlePasskeyLogin() {
    setError("");
    setLoading(true);
    try {
      finishLogin(await api.auth.loginWithPasskey());
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

  async function handleForgotSubmit(e: FormEvent) {
    e.preventDefault();
    setFpError("");
    setFpLoading(true);
    try {
      await api.auth.requestPasswordReset(fpEmail);
      setFpSent(true);
    } catch (err) {
      setFpError(getErrorMessage(err));
    } finally {
      setFpLoading(false);
    }
  }

  if (location.pathname.startsWith("/forgot-password")) {
    if (fpSent) {
      return (
        <>
          <p style={{ textAlign: "center", color: "#a0aec0", fontSize: "0.9rem", margin: "0 0 24px" }}>
            {t("auth:forgotPassword.sentDescription")}
          </p>
          <p className={styles.link}>
            <Link to="/login">{t("auth:forgotPassword.backToLoginLink")}</Link>
          </p>
        </>
      );
    }
    return (
      <>
        <p className={styles.description}>{t("auth:forgotPassword.description")}</p>
        <form onSubmit={handleForgotSubmit} className={styles.form}>
          <label className={styles.label}>
            {t("auth:forgotPassword.emailLabel")}
            <input
              type="email"
              value={fpEmail}
              onChange={(e) => setFpEmail(e.target.value)}
              className={styles.input}
              required
              autoFocus
            />
          </label>
          {fpError && <p className={styles.error}>{fpError}</p>}
          <button type="submit" className={styles.button} disabled={fpLoading}>
            {fpLoading ? t("auth:forgotPassword.sending") : t("auth:forgotPassword.submit")}
          </button>
        </form>
        <p className={styles.link}>
          <Link to="/login">{t("auth:forgotPassword.backToLoginLink")}</Link>
        </p>
      </>
    );
  }

  if (pendingToken) {
    return (
      <>
        <h3 className={styles.subtitle}>{t("auth:totpVerify.title")}</h3>
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
      </>
    );
  }

  return (
    <>
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
        <Turnstile siteKey={turnstileSiteKey} onToken={setTurnstileToken} />
        {error && <p className={styles.error}>{error}</p>}
        <button
          type="submit"
          className={styles.button}
          disabled={loading || (!!turnstileSiteKey && !turnstileToken)}
        >
          {loading ? t("auth:login.submitting") : t("auth:login.submit")}
        </button>
        <button type="button" className={styles.button} disabled={loading || !window.PublicKeyCredential} onClick={handlePasskeyLogin}>
          {t("auth:login.passkeySubmit")}
        </button>
      </form>
      <p className={styles.link}>
        {t("auth:login.forgotPasswordPrefix")} <Link to="/forgot-password">{t("auth:login.forgotPasswordLink")}</Link>
      </p>
    </>
  );
}
