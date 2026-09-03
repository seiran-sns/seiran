import { Fragment } from "react";
import { Link, NavLink, useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useAuth } from "../../contexts/AuthContext";
import { useSiteMeta } from "../../contexts/SiteMetaContext";
import { useStreamingContext } from "../../contexts/StreamingContext";
import { canAccessAdminPage } from "../../lib/roles";
import TwemojiEmoji from "../common/TwemojiEmoji";
import styles from "./AppShell.module.css";

interface NavItem {
  to: string;
  icon: string;
  labelKey: string;
  badge?: number;
}

const NAV_ITEMS: NavItem[] = [
  { to: "/", icon: "🏠", labelKey: "leftNav.home" },
  { to: "/search", icon: "🔍", labelKey: "leftNav.search" },
  { to: "/notifications", icon: "🔔", labelKey: "leftNav.notifications" },
  { to: "/messages", icon: "✉️", labelKey: "leftNav.messages" },
  { to: "/settings/lists", icon: "📋", labelKey: "leftNav.lists" },
  { to: "/settings", icon: "⚙️", labelKey: "leftNav.settings" },
];

interface LeftNavProps {
  onCompose: () => void;
  onOpenTarget: () => void;
  onItemClick?: () => void;
}

export default function LeftNav({ onCompose, onOpenTarget, onItemClick }: LeftNavProps) {
  const { t } = useTranslation();
  const { user, logout } = useAuth();
  const site = useSiteMeta();
  const { dmUnreadCount, followRequestCount } = useStreamingContext();
  const location = useLocation();

  const baseItems = NAV_ITEMS.map((item) => {
    if (item.to === "/messages") return { ...item, badge: dmUnreadCount };
    if (item.to === "/settings") return { ...item, badge: followRequestCount };
    return item;
  });
  const navItems = canAccessAdminPage(user?.role)
    ? [...baseItems, { to: "/admin", icon: "🛡️", labelKey: "leftNav.admin" }]
    : baseItems;

  function handleLogout() {
    onItemClick?.();
    // preserveRedirect: false により、未認証化を検知したRequireAuthが
    // `?redirect=`無しの`/login`へ遷移する（明示的なログアウトなのでホームへ
    // 戻したい。自分でnavigate()も呼ぶと、react-router-dom v7でのnavigate()の
    // transitionラップとRequireAuthの再描画が競合し、redirect付与が
    // 意図せず勝ってしまうことがあるため、ここでは呼ばずRequireAuthに委ねる）。
    logout({ preserveRedirect: false });
  }


  return (
    <nav className={styles.leftNav}>
      <div className={styles.logo}>
        {site.iconUrl && <img src={site.iconUrl} alt="" className={styles.logoIcon} />}
        <span className={styles.logoText}>{site.name}</span>
      </div>

      <ul className={styles.navList}>
        {navItems.map((item) => (
          <Fragment key={item.to}>
            <li>
              <NavLink
                to={item.to}
                end={item.to === "/"}
                className={({ isActive }) =>
                  `${styles.navLink} ${isActive ? styles.navLinkActive : ""}`
                }
                onClick={() => onItemClick?.()}
              >
                <TwemojiEmoji emoji={item.icon} className={styles.navIcon} />
                <span className={styles.navLabel}>{t(`nav:${item.labelKey}`)}</span>
                <span className={styles.navTooltip}>{t(`nav:${item.labelKey}`)}</span>
                {!!item.badge && <span className={styles.navBadge}>{item.badge > 99 ? "99+" : item.badge}</span>}
              </NavLink>
            </li>
            {item.to === "/search" && (
              <li>
                <button
                  type="button"
                  className={styles.navLink}
                  onClick={() => {
                    onItemClick?.();
                    onOpenTarget();
                  }}
                >
                  <TwemojiEmoji emoji="🔗" className={styles.navIcon} />
                  <span className={styles.navLabel}>{t("nav:leftNav.openTarget")}</span>
                  <span className={styles.navTooltip}>{t("nav:leftNav.openTarget")}</span>
                </button>
              </li>
            )}
          </Fragment>
        ))}
      </ul>

      <button
        className={styles.composeBtn}
        onClick={() => {
          onItemClick?.();
          onCompose();
        }}
      >
        <TwemojiEmoji emoji="✏️" className={styles.navIcon} />
        <span className={styles.navLabel}>{t("nav:leftNav.composeLabel")}</span>
        <span className={styles.navTooltip}>{t("nav:leftNav.composeLabel")}</span>
      </button>

      <div className={styles.navFooter}>
        {user ? (
          <>
            <Link
              to={`/@${user.username}`}
              className={styles.userChip}
              onClick={() => onItemClick?.()}
              title={t("nav:leftNav.profileTitle")}
            >
              <span className={styles.userAvatar}>
                {user.avatar_url ? (
                  <img src={user.avatar_url} alt="" className={styles.userAvatarImg} />
                ) : (
                  user.username[0]?.toUpperCase() ?? "?"
                )}
              </span>
              <span className={styles.navLabel}>@{user.username}</span>
            </Link>
            <button className={styles.logoutBtn} onClick={handleLogout} title={t("nav:leftNav.logoutTitle")}>
              ⏻
            </button>
          </>
        ) : (
          // 未ログイン状態でポスト詳細等を閲覧中の場合、現在の画面を`redirect`に載せてログインへ誘導する。
          <Link
            to={`/login?redirect=${encodeURIComponent(location.pathname + location.search)}`}
            className={styles.loginBtn}
            onClick={() => onItemClick?.()}
            title={t("nav:leftNav.loginTitle")}
          >
            {/* 左メニューがアイコンのみ幅（769〜900px）に畳まれた際、ラベルが消えて
                空ボタンになるのを防ぐアイコン。通常幅・モバイルドロワーでは非表示。 */}
            <TwemojiEmoji emoji="🔑" className={styles.loginIcon} />
            <span className={styles.navLabel}>{t("nav:leftNav.loginLabel")}</span>
          </Link>
        )}
      </div>
    </nav>
  );
}
