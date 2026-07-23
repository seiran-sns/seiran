import { ReactNode, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLocation } from "react-router-dom";
import { Note } from "../../api/client";
import { useStreamingContext } from "../../contexts/StreamingContext";
import Modal from "../common/Modal";
import PostComposer from "../note/PostComposer";
import LeftNav from "./LeftNav";
import styles from "./AppShell.module.css";

interface AppShellProps {
  /** 中央ペイン（メインコンテンツストリーム）。 */
  center: ReactNode;
  /** 右ペイン（動的コンテキスト領域）。省略時は非表示。 */
  right?: ReactNode;
  /** 投稿完了時のコールバック（ホーム画面が新規ノートを先頭に差し込むのに使う）。 */
  onPosted?: (note: Note) => void;
}

export default function AppShell({ center, right, onPosted }: AppShellProps) {
  const { t } = useTranslation();
  const location = useLocation();
  const { dmUnreadCount } = useStreamingContext();
  const [composeOpen, setComposeOpen] = useState(false);
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);

  // ページ移動時にモバイルメニューを自動で閉じる
  useEffect(() => {
    setMobileMenuOpen(false);
  }, [location.pathname, location.search]);

  return (
    <div className={styles.shell}>
      {/* PC表示用の左メニュー */}
      <div className={styles.desktopLeftNav}>
        <LeftNav onCompose={() => setComposeOpen(true)} />
      </div>

      <main className={styles.center}>{center}</main>

      <aside className={styles.right}>{right}</aside>

      {/* スマホ表示用フローティングメニューボタン */}
      <button
        className={styles.floatingMenuBtn}
        onClick={() => setMobileMenuOpen(true)}
        aria-label={t("nav:leftNav.openMenu")}
        title={t("nav:leftNav.openMenu")}
      >
        <span className={styles.floatingMenuIcon}>☰</span>
        {dmUnreadCount > 0 && (
          <span className={styles.floatingMenuBadge}>
            {dmUnreadCount > 99 ? "99+" : dmUnreadCount}
          </span>
        )}
      </button>

      {/* スマホ表示用フローティング投稿ボタン */}
      <button
        className={styles.floatingComposeBtn}
        onClick={() => setComposeOpen(true)}
        aria-label={t("nav:appShell.composeModalTitle")}
        title={t("nav:appShell.composeModalTitle")}
      >
        <span className={styles.floatingComposeIcon}>✏️</span>
      </button>

      {/* スマホ表示用モバイルドロワーメニュー */}
      {mobileMenuOpen && (
        <div className={styles.mobileBackdrop} onClick={() => setMobileMenuOpen(false)}>
          <div className={styles.mobileDrawer} onClick={(e) => e.stopPropagation()}>
            <div className={styles.mobileDrawerHeader}>
              <span className={styles.mobileDrawerTitle}>{t("nav:leftNav.menuTitle")}</span>
              <button
                className={styles.mobileDrawerCloseBtn}
                onClick={() => setMobileMenuOpen(false)}
                aria-label={t("nav:leftNav.closeMenu")}
              >
                ✕
              </button>
            </div>
            <LeftNav
              onCompose={() => {
                setMobileMenuOpen(false);
                setComposeOpen(true);
              }}
              onItemClick={() => setMobileMenuOpen(false)}
            />
          </div>
        </div>
      )}

      <Modal open={composeOpen} onClose={() => setComposeOpen(false)} title={t("nav:appShell.composeModalTitle")}>
        <PostComposer
          autoFocus
          onPosted={(note) => {
            setComposeOpen(false);
            onPosted?.(note);
          }}
        />
      </Modal>
    </div>
  );
}
