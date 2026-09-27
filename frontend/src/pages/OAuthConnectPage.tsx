import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import type { OAuthAuthorizeParams } from "../api/auth";
import styles from "./Auth.module.css";

type Phase = "loading" | "confirm" | "authorizing" | "oob" | "error";

/**
 * Mastodon 互換 OAuth の承認画面。バックエンドの `GET /oauth/authorize` がクライアント登録と
 * redirect_uri を検証したうえで、同じクエリを付けてここへリダイレクトする。「承認する」で
 * `POST /api/oauth/authorize` を通常の Bearer 認証で呼び、返ってきたリダイレクト先（登録済みの
 * redirect_uri に認可コードを付けたもの）へ遷移する。OOB の場合はコードを画面に表示する。
 * アプリ名は URL のクエリではなくサーバーの登録内容から引く（他のアプリを騙れないように）。
 */
export default function OAuthConnectPage() {
  const { t } = useTranslation();
  const [searchParams] = useSearchParams();
  const [phase, setPhase] = useState<Phase>("loading");
  const [appName, setAppName] = useState("");
  const [code, setCode] = useState("");
  const [error, setError] = useState("");

  const params = useMemo<OAuthAuthorizeParams | null>(() => {
    const clientId = searchParams.get("client_id");
    const redirectUri = searchParams.get("redirect_uri");
    if (!clientId || !redirectUri) return null;
    const optional = (key: string) => searchParams.get(key) ?? undefined;
    return {
      client_id: clientId,
      redirect_uri: redirectUri,
      response_type: optional("response_type"),
      scope: optional("scope"),
      state: optional("state"),
      code_challenge: optional("code_challenge"),
      code_challenge_method: optional("code_challenge_method"),
    };
  }, [searchParams]);

  useEffect(() => {
    if (!params) {
      setError(t("miauth:oauth.invalidRequest"));
      setPhase("error");
      return;
    }
    let cancelled = false;
    api.oauth
      .appInfo(params.client_id)
      .then((info) => {
        if (cancelled) return;
        setAppName(info.name);
        setPhase("confirm");
      })
      .catch(() => {
        if (cancelled) return;
        setError(t("miauth:oauth.invalidRequest"));
        setPhase("error");
      });
    return () => {
      cancelled = true;
    };
  }, [params, t]);

  async function handleAuthorize() {
    if (!params) return;
    setPhase("authorizing");
    setError("");
    try {
      const res = await api.oauth.authorize(params);
      if (res.redirectUrl) {
        window.location.href = res.redirectUrl;
        return;
      }
      setCode(res.code);
      setPhase("oob");
    } catch (err) {
      setError(getErrorMessage(err));
      setPhase("confirm");
    }
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("miauth:connect.title")}</h2>
        {phase === "oob" && (
          <>
            <p className={styles.description}>{t("miauth:oauth.oobDescription")}</p>
            <input className={styles.input} readOnly value={code} onFocus={(e) => e.target.select()} />
          </>
        )}
        {phase === "error" && <p className={styles.error}>{error}</p>}
        {(phase === "confirm" || phase === "authorizing") && (
          <>
            <p className={styles.description}>
              {t("miauth:connect.description", { appName: appName || t("miauth:connect.unknownApp") })}
            </p>
            {error && <p className={styles.error}>{error}</p>}
            <button
              type="button"
              className={styles.button}
              disabled={phase === "authorizing"}
              onClick={handleAuthorize}
            >
              {phase === "authorizing" ? t("miauth:connect.authorizing") : t("miauth:connect.submit")}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
