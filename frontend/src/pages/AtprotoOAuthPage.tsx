import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import type { AtprotoOAuthRequestInfo } from "../api/types";
import { useAuth } from "../contexts/AuthContext";
import authStyles from "./Auth.module.css";
import styles from "./AtprotoOAuth.module.css";

type Phase = "loading" | "confirm" | "submitting" | "error";

/** 権限ごとの説明。ここに無い権限はサーバーが付与しないので出てこない。 */
const SCOPE_KEYS: Record<string, string> = {
  atproto: "atprotoOAuth.scopeAtproto",
  "transition:generic": "atprotoOAuth.scopeGeneric",
  "transition:email": "atprotoOAuth.scopeEmail",
};

/**
 * AT Protocol OAuth の承認画面。バックエンドの `GET /oauth/atproto/authorize` が要求を確かめて
 * ここへ送る。許可・拒否のどちらでも、サーバーが返したクライアントの戻り先へ遷移する。
 * クライアント名・ロゴは自己申告なので、`client_id` のホスト名を必ず並べて出す。
 */
export default function AtprotoOAuthPage() {
  const { t } = useTranslation();
  const { logout } = useAuth();
  const [searchParams] = useSearchParams();
  const requestUri = searchParams.get("request_uri");
  const [phase, setPhase] = useState<Phase>("loading");
  const [info, setInfo] = useState<AtprotoOAuthRequestInfo | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!requestUri) {
      setError(t("miauth:atprotoOAuth.invalidRequest"));
      setPhase("error");
      return;
    }
    let cancelled = false;
    api.atprotoOAuth
      .request(requestUri)
      .then((res) => {
        if (cancelled) return;
        setInfo(res);
        setPhase("confirm");
      })
      .catch((e) => {
        if (cancelled) return;
        setError(getErrorMessage(e));
        setPhase("error");
      });
    return () => {
      cancelled = true;
    };
  }, [requestUri, t]);

  async function decide(approve: boolean) {
    if (!requestUri) return;
    setPhase("submitting");
    setError("");
    try {
      const res = approve
        ? await api.atprotoOAuth.approve(requestUri)
        : await api.atprotoOAuth.deny(requestUri);
      window.location.href = res.redirect_url;
    } catch (e) {
      setError(getErrorMessage(e));
      setPhase("confirm");
    }
  }

  const appName = info?.client_name || info?.client_host || "";

  return (
    <div className={authStyles.container}>
      <div className={authStyles.card}>
        <h1 className={authStyles.title}>{t("common:appName")}</h1>
        <h2 className={authStyles.subtitle}>{t("miauth:atprotoOAuth.title")}</h2>
        {phase === "loading" && <p className={authStyles.description}>{t("common:loading")}</p>}
        {phase === "error" && <p className={authStyles.error}>{error}</p>}
        {info && (phase === "confirm" || phase === "submitting") && (
          <>
            <div className={styles.client}>
              {info.logo_uri && <img className={styles.logo} src={info.logo_uri} alt="" />}
              <div className={styles.clientText}>
                <span className={styles.clientName}>{appName}</span>
                <span className={styles.clientHost}>
                  {t("miauth:atprotoOAuth.clientHost", { host: info.client_host })}
                </span>
              </div>
            </div>
            <p className={styles.note}>{t("miauth:atprotoOAuth.selfDeclaredNote")}</p>

            <p className={styles.account}>
              {t("miauth:atprotoOAuth.account", { handle: info.handle })}
            </p>
            {!info.login_hint_matches && (
              <div className={styles.warning}>
                <p>{t("miauth:atprotoOAuth.loginHintMismatch", { hint: info.login_hint })}</p>
                <button
                  type="button"
                  className={styles.secondaryButton}
                  onClick={() => logout()}
                  disabled={phase === "submitting"}
                >
                  {t("miauth:atprotoOAuth.switchAccount")}
                </button>
              </div>
            )}

            <p className={styles.scopeTitle}>{t("miauth:atprotoOAuth.scopeTitle")}</p>
            <ul className={styles.scopes}>
              {info.scopes
                .filter((s) => SCOPE_KEYS[s])
                .map((s) => (
                  <li key={s}>{t(`miauth:${SCOPE_KEYS[s]}`, { handle: info.handle })}</li>
                ))}
            </ul>

            {error && <p className={authStyles.error}>{error}</p>}
            <div className={styles.actions}>
              <button
                type="button"
                className={authStyles.button}
                disabled={phase === "submitting"}
                onClick={() => decide(true)}
              >
                {phase === "submitting"
                  ? t("miauth:connect.authorizing")
                  : t("miauth:atprotoOAuth.approve")}
              </button>
              <button
                type="button"
                className={styles.secondaryButton}
                disabled={phase === "submitting"}
                onClick={() => decide(false)}
              >
                {t("miauth:atprotoOAuth.deny")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
