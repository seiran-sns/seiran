import { useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import styles from "./Auth.module.css";

type Phase = "confirm" | "authorizing" | "done" | "error";

/**
 * バックエンドの `is_valid_callback`（denylist方式）と同じ考え方の最終確認。
 * サーバー側の `/miauth/:sessionId` 入口で既に検証済みだが、ここは実際に
 * `window.location.href` で遷移する直前の防御的な二重チェック。
 */
function isSafeCallback(url: string): boolean {
  try {
    const protocol = new URL(url).protocol;
    return !["http:", "javascript:", "data:", "vbscript:", "file:"].includes(protocol);
  } catch {
    return false;
  }
}

function buildCallbackUrl(callback: string, sessionId: string): string {
  try {
    const url = new URL(callback);
    url.searchParams.set("session", sessionId);
    return url.toString();
  } catch {
    // URL API がうまく扱えない特殊なスキームへのフォールバック（単純な文字列結合）。
    return callback.includes("?")
      ? `${callback}&session=${encodeURIComponent(sessionId)}`
      : `${callback}?session=${encodeURIComponent(sessionId)}`;
  }
}

/**
 * MiAuth 認可確認画面（issue: Aria 等サードパーティクライアントのログイン後、
 * `GET /miauth/:sessionId` が `RequireAuth` 経由でこのページへリダイレクトする）。
 * 「承認する」押下で `POST /api/miauth/:sessionId/authorize` を通常の Bearer 認証で呼び、
 * 成功したら callback（ネイティブアプリのカスタム URI スキーム等）へ遷移する。
 */
export default function MiAuthConnectPage() {
  const { t } = useTranslation();
  const { sessionId } = useParams<{ sessionId: string }>();
  const [searchParams] = useSearchParams();
  const [phase, setPhase] = useState<Phase>("confirm");
  const [error, setError] = useState("");

  const appName = searchParams.get("name") || t("miauth:connect.unknownApp");
  const callback = searchParams.get("callback");

  async function handleAuthorize() {
    if (!sessionId) return;
    setPhase("authorizing");
    setError("");
    try {
      await api.miauth.authorize(sessionId);
      if (callback && isSafeCallback(callback)) {
        window.location.href = buildCallbackUrl(callback, sessionId);
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
        {phase === "done" ? (
          <p className={styles.description}>{t("miauth:connect.doneDescription")}</p>
        ) : (
          <>
            <p className={styles.description}>
              {t("miauth:connect.description", { appName })}
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
