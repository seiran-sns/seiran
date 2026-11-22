import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../api/client";
import type { AccountMigration } from "../api/users";
import { useMigrationImportActive } from "../hooks/useMigrationImport";
import styles from "./FollowImportSettings.module.css";

const POLL_INTERVAL_MS = 5000;

/**
 * 設定画面「🚚 インポート」の先頭に出す、Bluesky からの転入の取り込み状況
 * （`docs/account_migration.md` 3節）。取り込みは PLC 提出後に裏で進み、利用者はその間も
 * アカウントを使えるので、進み具合と打ち切りボタンはここにまとめる（取り込み中はメニューの
 * ⚙️設定と🚚インポートに明滅する赤丸を出して誘う）。
 */
export default function MigrationImportSection() {
  const { t } = useTranslation();
  const active = useMigrationImportActive();
  const [migration, setMigration] = useState<AccountMigration | null>(null);
  const [error, setError] = useState("");
  const [aborting, setAborting] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setMigration(await api.account.migration());
    } catch {
      // 一時的な失敗は次回のポーリングに任せる。
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    refresh();
    const timer = setInterval(refresh, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [active, refresh]);

  async function handleAbort() {
    if (!window.confirm(t("account:migrationImport.abortConfirm"))) return;
    setError("");
    setAborting(true);
    try {
      await api.account.abortMigrationImport();
      await refresh();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setAborting(false);
    }
  }

  if (!active || !migration) return null;

  const statusLabel = (() => {
    switch (migration.status) {
      case "importing_data":
        return t("account:migrationImport.statusRecords");
      case "importing_blobs":
        return t("account:migrationImport.statusBlobs");
      case "deactivating_source":
        return t("account:migrationImport.statusDeactivating");
      default:
        return null;
    }
  })();
  const blobsDone = migration.blobs_imported + migration.blobs_given_up;
  const canAbort =
    !migration.aborted && (migration.status === "importing_data" || migration.status === "importing_blobs");

  return (
    <div className={styles.section}>
      <h3 className={styles.sectionTitle}>{t("account:migrationImport.title")}</h3>
      <p className={styles.description}>{t("account:migrationImport.description")}</p>
      <div className={styles.progress}>
        {statusLabel && <p className={styles.progressStatus}>{statusLabel}</p>}
        <p className={styles.progressLine}>
          {t("account:migrationImport.records", {
            done: migration.records_imported,
            total: migration.records_total,
          })}
        </p>
        <p className={styles.progressLine}>
          {t("account:migrationImport.blobs", { done: blobsDone, total: migration.blobs_total })}
          {migration.blobs_given_up > 0 &&
            ` ${t("account:migrationImport.blobsGivenUp", { count: migration.blobs_given_up })}`}
        </p>
        {!migration.repo_refetched && !migration.aborted && (
          <p className={styles.progressLine}>{t("account:migrationImport.refetchPending")}</p>
        )}
        {migration.waiting_for_source && (
          <p className={styles.progressLine}>{t("account:migrationImport.waitingForSource")}</p>
        )}
        {migration.aborted && <p className={styles.progressLine}>{t("account:migrationImport.aborted")}</p>}
        {error && <p className={styles.error}>{error}</p>}
        {canAbort && (
          <>
            <p className={styles.description}>{t("account:migrationImport.abortWarning")}</p>
            <button type="button" className={styles.cancelButton} disabled={aborting} onClick={handleAbort}>
              {t("account:migrationImport.abort")}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
