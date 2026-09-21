import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api/client";
import type { MigrationStatusResponse } from "../api/migration";
import { useAuth } from "../contexts/AuthContext";
import { clearStoredMigrationRequest, loadStoredMigrationRequest } from "./migrationStorage";
import { migrationStepLabel } from "./migrationStatusLabels";
import styles from "./Auth.module.css";

const POLL_INTERVAL_MS = 3000;

/**
 * ログイン済み（JWT取得済み）だが`migration_status`が`completed`でないアカウントに表示する
 * 専用画面。`App.tsx::AppRoutes`が`user.migration_status`を見て、`SuspendedAccountPage`と
 * 同型で他の全ルートをバイパスしてこれだけを表示する。
 *
 * `submit-plc-token`成功時点でJWTは既に発行済みのため、`GET /api/auth/me`を
 * ポーリングして`migration_status`が消える（=completedになる）のを待つのが本体だが、
 * `MigratePanel`がクリアせず残した`localStorage`のmigration_token/idがまだ使えるため
 * （`docs/account_migration.md`参照）、`/api/migration/:id/status`も並行してポーリングし
 * `importing_data`中の取り込み件数/全体件数を表示する（`MigratePanel`の状態画面と同じ表示）。
 * 完了を検知した時点でこのlocalStorageをクリアする。
 */
export default function MigrationImportingPage() {
  const { t } = useTranslation();
  const { user, logout, login } = useAuth();

  const [stored] = useState(() => loadStoredMigrationRequest());
  const [statusData, setStatusData] = useState<MigrationStatusResponse | null>(null);

  const fetchMigrationStatus = useCallback(async () => {
    if (!stored) return;
    try {
      const res = await api.migration.status(stored.id, stored.token);
      setStatusData(res);
    } catch {
      // 進捗表示は補助情報のため、失敗しても`/api/auth/me`ポーリングの方に任せる。
    }
  }, [stored]);

  const pollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    fetchMigrationStatus();
    return () => {
      if (pollTimer.current) clearTimeout(pollTimer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!stored) return;
    pollTimer.current = setTimeout(fetchMigrationStatus, POLL_INTERVAL_MS);
    return () => {
      if (pollTimer.current) clearTimeout(pollTimer.current);
    };
  }, [stored, statusData, fetchMigrationStatus]);

  useEffect(() => {
    const timer = setInterval(() => {
      api.auth
        .me()
        .then((freshUser) => {
          if (!freshUser.migration_status) clearStoredMigrationRequest();
          login(freshUser.token, freshUser);
        })
        .catch(() => {
          // 一時的な失敗は無視して次回ポーリングに任せる（AuthContextの401処理に委ねる）。
        });
    }, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [login]);

  const stepLabel = migrationStepLabel(t, statusData);

  return (
    <div className={styles.container}>
      <div className={styles.card}>
        <h1 className={styles.title}>{t("common:appName")}</h1>
        <h2 className={styles.subtitle}>{t("auth:migrationImporting.title")}</h2>
        {user?.username && <p>@{user.username}</p>}
        <p className={styles.description}>
          <span className={styles.pendingHighlight}>
            {stepLabel || t("auth:migrationImporting.description")}
          </span>
        </p>
        <button type="button" className={styles.button} onClick={() => logout({ preserveRedirect: false })}>
          {t("auth:migrationImporting.logout")}
        </button>
      </div>
    </div>
  );
}
