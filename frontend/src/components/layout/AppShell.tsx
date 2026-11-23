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
import { useMigrationImportActive, useMigrationImportWatcher } from "../../hooks/useMigrationImport";
import styles from "./AppShell.module.css";
import { useBackgroundImports } from "../../contexts/BackgroundImportsContext";

interface AppShellProps {
  /** 中央ペイン（メインコンテンツストリーム）。 */
  center: ReactNode;
  /** 右ペイン（動的コンテキスト領域）。省略時は非表示。 */
  right?: ReactNode;
  /** 投稿完了時のコールバック（ホーム画面が新規ノートを先頭に差し込むのに使う）。 */
  onPosted?: (note: Note) => void;
  /** RouterがDOMを差し替えてwindow.scrollYを変える前に、現在画面の状態を保存する。 */
  onBeforeNavigate?: () => void;
  /** 中央と右のペインが横に並ばない幅（1220px以下）で、片方だけを出す。省略時は両方を縦に積む。
   * 一覧（右）と本体（中央）を行き来する画面（DM）で、縦に積むと本体の下に一覧が埋もれるため。 */
  narrowPane?: "center" | "right";
}

export default function AppShell({ center, right, onPosted, onBeforeNavigate, narrowPane }: AppShellProps) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const location = useLocation();
  const { dmUnreadCount, followRequestCount, unread, openReportCount } = useStreamingContext();
  // ☰ のバッジはメニューの中の項目（通知・メッセージ・設定・管理）のバッジの合計。メニューを
  // 開くまで気づけない件数を、閉じたままでも見えるようにする（通知は🔔にも出すが、メニューの
  // 中の「通知」にもバッジがあるので合計に含める）。
  const menuBadgeCount = unread + dmUnreadCount + followRequestCount + openReportCount;
  // ☰ の明滅ドットも、メニューの中の項目（設定・管理）のドットをまとめたもの。
  const { followImportActive, emojiImportActive } = useBackgroundImports();
  const [composeOpen, setComposeOpen] = useState(false);
  const [openTargetOpen, setOpenTargetOpen] = useState(false);
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);
  useMigrationImportWatcher();
  const migrationImportActive = useMigrationImportActive();

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
      className={`${styles.shell} ${
        narrowPane === "center" ? styles.narrowCenterOnly : narrowPane === "right" ? styles.narrowRightOnly : ""
      }`}
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

      <aside className={styles.right}>
        <div className={styles.rightScroll}>{right}</div>
      </aside>

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
          {menuBadgeCount > 0 && (
            <span className={styles.floatingMenuBadge}>
              {menuBadgeCount > 99 ? "99+" : menuBadgeCount}
            </span>
          )}
          {menuBadgeCount === 0 && (migrationImportActive || followImportActive || emojiImportActive) && (
            <span className={styles.floatingMenuDot} aria-hidden="true" />
          )}
        </button>

        {/* 未ログインでは、ホーム・通知・検索・投稿はどれもログインを求めるだけなので出さない
            （メニューからログイン・「このサイトについて」へ行ける）。 */}
        {user && (
          <>
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
              {unread > 0 && (
                <span className={styles.floatingMenuBadge}>{unread > 99 ? "99+" : unread}</span>
              )}
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
          </>
        )}
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
