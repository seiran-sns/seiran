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

/**
 * Bluesky への配送が転入の取り込み完了まで保留されるか。レコードを取り込み終えるまで
 * （`importing_data`）、サーバーは ATP への書き込みを積んでおき、取り込み完了時に流す
 * （`docs/account_migration.md` 3節）。投稿フォームの Bluesky ボタンに注意の印を出すのに使う。
 */
export function useBskyDeliveryDeferred(): boolean {
  const { user } = useAuth();
  return user?.migration_status === "importing_data";
}

const WATCH_INTERVAL_MS = 60_000;

/**
 * 取り込みが続いている間、状態を定期的に確かめてログイン中のユーザー情報の `migration_status`
 * に写す。終わったら消す（バッジが消える）。`AppShell` で1回だけ呼ぶ。
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
          if (res && IMPORTING_STATUSES.includes(res.status)) {
            updateUserRef.current({ migration_status: res.status });
            return;
          }
          updateUserRef.current({ migration_status: null });
        })
        .catch(() => {
          // 一時的な失敗は次回に任せる。
        });
    }, WATCH_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [active]);
}
