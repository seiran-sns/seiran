import { createContext, useCallback, useContext, useEffect, useState } from "react";
import { api } from "../api/client";
import { getAdminTopics } from "../lib/roles";
import { useAuth } from "./AuthContext";

/**
 * 裏で続くインポート（フォローインポート・絵文字インポート）が進行中か。メニューの明滅ドットに使う。
 *
 * 開いたとき（ログインしたとき）に1回確かめ、進行中の間だけ60秒ごとに確かめ直して、終わったら
 * ドットを消す。進捗画面（設定 > インポート、管理 > 絵文字）を開いている間はその画面が細かく
 * 取り直した状態を `report*` で伝え、開始・完了をすぐ反映する。
 */
interface BackgroundImportsValue {
  followImportActive: boolean;
  emojiImportActive: boolean;
  reportFollowImportStatus: (status: string) => void;
  reportEmojiImportStatus: (status: string) => void;
}

const BackgroundImportsContext = createContext<BackgroundImportsValue>({
  followImportActive: false,
  emojiImportActive: false,
  reportFollowImportStatus: () => {},
  reportEmojiImportStatus: () => {},
});

const WATCH_INTERVAL_MS = 60_000;
const followImportRunning = (status: string) => status === "running";
const emojiImportRunning = (status: string) => status === "preparing" || status === "running";

/** `active` の間だけ `check` を定期的に呼ぶ。`watchKey` が変わったとき（ログイン・ユーザーの
 * 切り替え）に1回呼ぶ。`watchKey` が null の間は何もしない。 */
function useWatch(watchKey: string | null, active: boolean, check: () => void) {
  const enabled = watchKey !== null;
  useEffect(() => {
    if (enabled) check();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [watchKey]);
  useEffect(() => {
    if (!enabled || !active) return;
    const timer = setInterval(check, WATCH_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [enabled, active, check]);
}

export function BackgroundImportsProvider({ children }: { children: React.ReactNode }) {
  const { user } = useAuth();
  const [followImportActive, setFollowImportActive] = useState(false);
  const [emojiImportActive, setEmojiImportActive] = useState(false);
  const canImportEmojis = getAdminTopics(user?.role).includes("emojis");

  const reportFollowImportStatus = useCallback((status: string) => {
    setFollowImportActive(followImportRunning(status));
  }, []);
  const reportEmojiImportStatus = useCallback((status: string) => {
    setEmojiImportActive(emojiImportRunning(status));
  }, []);

  const checkFollowImport = useCallback(() => {
    api.followImport
      .status()
      .then((s) => reportFollowImportStatus(s.status))
      .catch(() => {
        // 一時的な失敗は次回に任せる。
      });
  }, [reportFollowImportStatus]);
  const checkEmojiImport = useCallback(() => {
    api.admin
      .getEmojiImportStatus()
      .then((s) => reportEmojiImportStatus(s.status))
      .catch(() => {
        // 一時的な失敗は次回に任せる。
      });
  }, [reportEmojiImportStatus]);

  useWatch(user ? String(user.id) : null, followImportActive, checkFollowImport);
  useWatch(user && canImportEmojis ? String(user.id) : null, emojiImportActive, checkEmojiImport);

  // ログアウト・ロール変更で見られなくなったら消す。
  useEffect(() => {
    if (!user) setFollowImportActive(false);
    if (!canImportEmojis) setEmojiImportActive(false);
  }, [user, canImportEmojis]);

  return (
    <BackgroundImportsContext.Provider
      value={{ followImportActive, emojiImportActive, reportFollowImportStatus, reportEmojiImportStatus }}
    >
      {children}
    </BackgroundImportsContext.Provider>
  );
}

export function useBackgroundImports() {
  return useContext(BackgroundImportsContext);
}
