import { useTranslation } from "react-i18next";
import { useAuth } from "../contexts/AuthContext";
import styles from "./Auth.module.css";

/**
 * 凍結中のローカルユーザーに表示する専用画面。`App.tsx::AppRoutes` が
 * `user.is_suspended` を見て、他の全ルートをバイパスしてこれだけを表示する。
 * バックエンド側は `extract_auth` が `GET /api/auth/me` 以外の全APIを
 * `ACCOUNT_SUSPENDED` で拒否するため、ここではログアウト以外の操作を提供しない。
 */
export default function SuspendedAccountPage() {
  const { t } = useTranslation();
  const { user, logout } = useAuth();

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:suspended.title")}</h2>
        {user?.username && <p>@{user.username}</p>}
        <p>{t("auth:suspended.description")}</p>
        <button type="button" className={styles.button} onClick={() => logout({ preserveRedirect: false })}>
          {t("auth:suspended.logout")}
        </button>
      </div>
    </div>
  );
}
