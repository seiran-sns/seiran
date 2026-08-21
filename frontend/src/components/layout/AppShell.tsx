import { ReactNode, useEffect, useState } from "react";
import { flushSync } from "react-dom";
import { useTranslation } from "react-i18next";
import { Link, useLocation } from "react-router-dom";
import { Note } from "../../api/client";
import { useAuth } from "../../contexts/AuthContext";
import { useStreamingContext } from "../../contexts/StreamingContext";
import { refreshComposerDraft } from "../../lib/composerDraft";
import Modal from "../common/Modal";
import TwemojiEmoji from "../common/TwemojiEmoji";
import PostComposer from "../note/PostComposer";
import OpenTargetDialog from "../open/OpenTargetDialog";
import LeftNav from "./LeftNav";
import styles from "./AppShell.module.css";

interface AppShellProps {
  /** 中央ペイン（メインコンテンツストリーム）。 */
  center: ReactNode;
  /** 右ペイン（動的コンテキスト領域）。省略時は非表示。 */
  right?: ReactNode;
  /** 投稿完了時のコールバック（ホーム画面が新規ノートを先頭に差し込むのに使う）。 */
  onPosted?: (note: Note) => void;
  /** RouterがDOMを差し替えてwindow.scrollYを変える前に、現在画面の状態を保存する。 */
  onBeforeNavigate?: () => void;
}

export default function AppShell({ center, right, onPosted, onBeforeNavigate }: AppShellProps) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const location = useLocation();
  const { dmUnreadCount } = useStreamingContext();
  const [composeOpen, setComposeOpen] = useState(false);
  const [openTargetOpen, setOpenTargetOpen] = useState(false);
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);

  const closeCompose = () => {
    // Modal 内コンポーザを同期的にアンマウントしてから、常設のホーム上部
    // コンポーザへ保存済み下書きの再読込を依頼する。
    // setTimeout(0) では React の commit より先に再読込される場合がある。
    flushSync(() => setComposeOpen(false));
    if (user) {
      refreshComposerDraft({ mode: "compose", userId: user.id });
    }
  };

  // ページ移動時にモバイルメニューを自動で閉じる
  useEffect(() => {
    setMobileMenuOpen(false);
  }, [location.pathname, location.search]);

  return (
    <div
      className={styles.shell}
      onClickCapture={(event) => {
        if ((event.target as Element).closest("a[href]")) onBeforeNavigate?.();
      }}
    >
      {/* PC表示用の左メニュー */}
      <div className={styles.desktopLeftNav}>
        <LeftNav
          onCompose={() => setComposeOpen(true)}
          onOpenTarget={() => setOpenTargetOpen(true)}
          onItemClick={onBeforeNavigate}
        />
      </div>

      <main className={styles.center}>{center}</main>

      <aside className={styles.right}>{right}</aside>

      {/* スマホ表示用フローティングボタン群（5個をflexで均等配置し、狭い画面幅でも
          重なりやはみ出しが起きないようにする） */}
      <div className={styles.floatingNavBar}>
        <button
          className={styles.floatingMenuBtn}
          onClick={() => setMobileMenuOpen(true)}
          aria-label={t("nav:leftNav.openMenu")}
          title={t("nav:leftNav.openMenu")}
        >
          <TwemojiEmoji emoji="☰" className={styles.floatingMenuIcon} />
          {dmUnreadCount > 0 && (
            <span className={styles.floatingMenuBadge}>
              {dmUnreadCount > 99 ? "99+" : dmUnreadCount}
            </span>
          )}
        </button>

        {/* #180 */}
        <Link
          to="/"
          className={styles.floatingHomeBtn}
          onClick={() => onBeforeNavigate?.()}
          aria-label={t("nav:leftNav.home")}
          title={t("nav:leftNav.home")}
        >
          <TwemojiEmoji emoji="🏠" className={styles.floatingHomeIcon} />
        </Link>

        {/* #75 */}
        <Link
          to="/notifications"
          className={styles.floatingNotifBtn}
          onClick={() => onBeforeNavigate?.()}
          aria-label={t("nav:leftNav.notifications")}
          title={t("nav:leftNav.notifications")}
        >
          <TwemojiEmoji emoji="🔔" className={styles.floatingNotifIcon} />
        </Link>

        <Link
          to="/search"
          className={styles.floatingSearchBtn}
          onClick={() => onBeforeNavigate?.()}
          aria-label={t("nav:leftNav.search")}
          title={t("nav:leftNav.search")}
        >
          <TwemojiEmoji emoji="🔍" className={styles.floatingSearchIcon} />
        </Link>

        <button
          className={styles.floatingComposeBtn}
          onClick={() => setComposeOpen(true)}
          aria-label={t("nav:appShell.composeModalTitle")}
          title={t("nav:appShell.composeModalTitle")}
        >
          <TwemojiEmoji emoji="✏️" className={styles.floatingComposeIcon} />
        </button>
      </div>

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
              onOpenTarget={() => {
                setMobileMenuOpen(false);
                setOpenTargetOpen(true);
              }}
              onItemClick={() => setMobileMenuOpen(false)}
            />
          </div>
        </div>
      )}

      <Modal open={composeOpen} onClose={closeCompose} title={t("nav:appShell.composeModalTitle")}>
        <PostComposer
          autoFocus
          onPosted={(note) => {
            closeCompose();
            onPosted?.(note);
          }}
        />
      </Modal>
      <OpenTargetDialog
        open={openTargetOpen}
        onClose={() => setOpenTargetOpen(false)}
        onBeforeNavigate={onBeforeNavigate}
      />
    </div>
  );
}
