import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import styles from "./Auth.module.css";

const POLL_INTERVAL_MS = 3000;

/**
 * ログイン済み（JWT取得済み）だが`migration_status`が`completed`でないアカウントに表示する
 * 専用画面。`App.tsx::AppRoutes`が`user.migration_status`を見て、`SuspendedAccountPage`と
 * 同型で他の全ルートをバイパスしてこれだけを表示する。
 *
 * `submit-plc-token`成功時点でJWTは既に発行済みのため、ここでは`GET /api/auth/me`を
 * ポーリングして`migration_status`が消える（=completedになる）のを待つだけでよい。
 */
export default function MigrationImportingPage() {
  const { t } = useTranslation();
  const { user, logout, login } = useAuth();

  useEffect(() => {
    const timer = setInterval(() => {
      api.auth
        .me()
        .then((freshUser) => login(freshUser.token, freshUser))
        .catch(() => {
          // 一時的な失敗は無視して次回ポーリングに任せる（AuthContextの401処理に委ねる）。
        });
    }, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [login]);

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:migrationImporting.title")}</h2>
        {user?.username && <p>@{user.username}</p>}
        <p style={{ textAlign: "center", color: "#a0aec0", lineHeight: 1.6 }}>
          {t("auth:migrationImporting.description")}
        </p>
        <button type="button" className={styles.button} onClick={() => logout({ preserveRedirect: false })}>
          {t("auth:migrationImporting.logout")}
        </button>
      </div>
    </div>
  );
}
