import { useEffect, useRef } from "react";
import { api } from "../api/client";
import { useAuth } from "../contexts/AuthContext";

/** アカウント作成後、まだ完了していない DID 転入の状態（取り込みが裏で続いている）。 */
const IMPORTING_STATUSES = ["importing_data", "importing_blobs", "deactivating_source"];

/** Bluesky からの転入の取り込みが裏で続いているか（メニューのバッジ・設定 > インポートの表示用）。 */
export function useMigrationImportActive(): boolean {
  const { user } = useAuth();
  return !!user?.migration_status && IMPORTING_STATUSES.includes(user.migration_status);
}

const WATCH_INTERVAL_MS = 60_000;

/**
 * 取り込みが続いている間、終わったかを定期的に確かめ、終わったらログイン中のユーザー情報の
 * `migration_status` を消す（バッジが消える）。`AppShell` で1回だけ呼ぶ。
 */
export function useMigrationImportWatcher() {
  const active = useMigrationImportActive();
  const { updateUser } = useAuth();
  // `updateUser` は描画ごとに作り直されるので、依存に入れるとタイマーが毎回張り直される。
  const updateUserRef = useRef(updateUser);
  updateUserRef.current = updateUser;

  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => {
      api.account
        .migration()
        .then((res) => {
          if (res && IMPORTING_STATUSES.includes(res.status)) return;
          updateUserRef.current({ migration_status: null });
        })
        .catch(() => {
          // 一時的な失敗は次回に任せる。
        });
    }, WATCH_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [active]);
}
