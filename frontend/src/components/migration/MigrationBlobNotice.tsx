import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../../api/client";
import type { AccountMigration } from "../../api/users";
import { useAuth } from "../../contexts/AuthContext";
import styles from "./MigrationBlobNotice.module.css";

/** 転入の画像等の取り込み中・転入元の無効化中に、この案内を出す状態。 */
const NOTICE_STATUSES = ["importing_blobs", "deactivating_source"];
const POLL_INTERVAL_MS = 30_000;

/**
 * DID 転入の画像等（blob）の取り込み中に、中央ペインの上へ出す案内（`docs/account_migration.md`
 * 3節）。アカウントは使える状態なので画面は塞がず、進み具合・諦める期限・打ち切りボタンだけを出す。
 * ログイン時の`user.migration_status`で表示を始め、以後は`GET /api/account/migration`を
 * ポーリングして、完了したら消す。
 */
export default function MigrationBlobNotice() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const active = !!user?.migration_status && NOTICE_STATUSES.includes(user.migration_status);
  const [migration, setMigration] = useState<AccountMigration | null>(null);
  const [done, setDone] = useState(false);
  const [error, setError] = useState("");
  const [skipping, setSkipping] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const res = await api.account.migration();
      if (!res || !NOTICE_STATUSES.includes(res.status)) {
        setDone(true);
        return;
      }
      setMigration(res);
    } catch {
      // 一時的な失敗は次回のポーリングに任せる。
    }
  }, []);

  useEffect(() => {
    if (!active || done) return;
    refresh();
    const timer = setInterval(refresh, POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [active, done, refresh]);

  async function handleSkip() {
    if (!window.confirm(t("account:migrationBlobs.skipConfirm"))) return;
    setError("");
    setSkipping(true);
    try {
      await api.account.skipMigrationBlobs();
      await refresh();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setSkipping(false);
    }
  }

  if (!active || done || !migration) return null;

  if (migration.status === "deactivating_source") {
    return <div className={styles.notice}>{t("account:migrationBlobs.deactivating")}</div>;
  }
  return (
    <div className={styles.notice}>
      <p className={styles.text}>
        {t("account:migrationBlobs.progress", {
          done: migration.blobs_imported + migration.blobs_given_up,
          total: migration.blobs_total,
        })}
      </p>
      {migration.blob_import_deadline && (
        <p className={styles.sub}>
          {t("account:migrationBlobs.deadline", {
            time: new Date(migration.blob_import_deadline).toLocaleString(),
          })}
        </p>
      )}
      {error && <p className={styles.error}>{error}</p>}
      <button type="button" className={styles.button} onClick={handleSkip} disabled={skipping}>
        {t("account:migrationBlobs.skip")}
      </button>
    </div>
  );
}
