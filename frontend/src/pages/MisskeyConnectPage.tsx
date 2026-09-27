import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

type Phase = "loading" | "confirm" | "authorizing" | "done" | "error";

/**
 * バックエンドの `is_valid_callback`（denylist方式）と同じ考え方の最終確認
 * （`MiAuthConnectPage` の `isSafeCallback` と同一実装）。
 */
function isSafeCallback(url: string): boolean {
  try {
    const protocol = new URL(url).protocol;
    return !["http:", "javascript:", "data:", "vbscript:", "file:"].includes(protocol);
  } catch {
    return false;
  }
}

function buildCallbackUrl(callback: string, token: string): string {
  try {
    const url = new URL(callback);
    url.searchParams.set("token", token);
    return url.toString();
  } catch {
    return callback.includes("?")
      ? `${callback}&token=${encodeURIComponent(token)}`
      : `${callback}?token=${encodeURIComponent(token)}`;
  }
}

/**
 * Misskey 旧来の app 認証フロー（app/create → auth/session/generate → auth/session/userkey）の
 * 承認確認画面。`GET /auth/:token` がここへリダイレクトする。アプリ名は URL のクエリではなく
 * `GET /api/auth-sessions/:token`（サーバー登録内容）から引く（`OAuthConnectPage` と同じ理由、
 * 他のアプリへのなりすまし防止）。
 */
export default function MisskeyConnectPage() {
  const { t } = useTranslation();
  const { token } = useParams<{ token: string }>();
  const [phase, setPhase] = useState<Phase>("loading");
  const [appName, setAppName] = useState("");
  const [callbackUrl, setCallbackUrl] = useState<string | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!token) {
      setError(t("miauth:oauth.invalidRequest"));
      setPhase("error");
      return;
    }
    let cancelled = false;
    api.misskeyAppAuth
      .sessionInfo(token)
      .then((info) => {
        if (cancelled) return;
        setAppName(info.name);
        setCallbackUrl(info.callbackUrl);
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
  }, [token, t]);

  async function handleAuthorize() {
    if (!token) return;
    setPhase("authorizing");
    setError("");
    try {
      await api.misskeyAppAuth.authorize(token);
      if (callbackUrl && isSafeCallback(callbackUrl)) {
        window.location.href = buildCallbackUrl(callbackUrl, token);
        return;
      }
      setPhase("done");
    } catch (err) {
      setError(getErrorMessage(err));
      setPhase("error");
    }
  }

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("miauth:connect.title")}</h2>
        {phase === "error" && <p className={styles.error}>{error}</p>}
        {phase === "done" && (
          <p className={styles.description}>{t("miauth:connect.doneDescription")}</p>
        )}
        {(phase === "loading" || phase === "confirm" || phase === "authorizing") && (
          <>
            <p className={styles.description}>
              {t("miauth:connect.description", {
                appName: appName || t("miauth:connect.unknownApp"),
              })}
            </p>
            {error && <p className={styles.error}>{error}</p>}
            <button
              type="button"
              className={styles.button}
              disabled={phase === "loading" || phase === "authorizing"}
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
