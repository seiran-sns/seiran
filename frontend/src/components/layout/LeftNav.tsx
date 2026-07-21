import { NavLink, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useAuth } from "../../contexts/AuthContext";
import { useSiteMeta } from "../../contexts/SiteMetaContext";
import { useStreamingContext } from "../../contexts/StreamingContext";
import { isAdminRole } from "../../lib/roles";
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
];

interface LeftNavProps {
  onCompose: () => void;
  onItemClick?: () => void;
}

export default function LeftNav({ onCompose, onItemClick }: LeftNavProps) {
  const { t } = useTranslation();
  const { user, logout } = useAuth();
  const site = useSiteMeta();
  const navigate = useNavigate();
  const { dmUnreadCount } = useStreamingContext();

  const baseItems = NAV_ITEMS.map((item) =>
    item.to === "/messages" ? { ...item, badge: dmUnreadCount } : item
  );
  const navItems = isAdminRole(user?.role)
    ? [...baseItems, { to: "/admin", icon: "🛡️", labelKey: "leftNav.admin" }]
    : baseItems;

  function handleLogout() {
    onItemClick?.();
    logout();
    navigate("/login");
  }

  function handleProfileClick() {
    onItemClick?.();
    if (user?.username) {
      navigate(`/@${user.username}`);
    }
  }

  return (
    <nav className={styles.leftNav}>
      <div className={styles.logo}>
        {site.iconUrl && <img src={site.iconUrl} alt="" className={styles.logoIcon} />}
        <span className={styles.logoText}>{site.name}</span>
      </div>

      <ul className={styles.navList}>
        {navItems.map((item) => (
          <li key={item.to}>
            <NavLink
              to={item.to}
              end={item.to === "/"}
              className={({ isActive }) =>
                `${styles.navLink} ${isActive ? styles.navLinkActive : ""}`
              }
              onClick={() => onItemClick?.()}
            >
              <span className={styles.navIcon}>{item.icon}</span>
              <span className={styles.navLabel}>{t(`nav:${item.labelKey}`)}</span>
              {!!item.badge && <span className={styles.navBadge}>{item.badge > 99 ? "99+" : item.badge}</span>}
            </NavLink>
          </li>
        ))}
      </ul>

      <button
        className={styles.composeBtn}
        onClick={() => {
          onItemClick?.();
          onCompose();
        }}
      >
        <span className={styles.navIcon}>✏️</span>
        <span className={styles.navLabel}>{t("nav:leftNav.composeLabel")}</span>
      </button>

      <div className={styles.navFooter}>
        <button
          className={styles.userChip}
          onClick={handleProfileClick}
          title={t("nav:leftNav.profileTitle")}
        >
          <span className={styles.userAvatar}>
            {user?.avatar_url ? (
              <img src={user.avatar_url} alt="" className={styles.userAvatarImg} />
            ) : (
              user?.username?.[0]?.toUpperCase() ?? "?"
            )}
          </span>
          <span className={styles.navLabel}>@{user?.username}</span>
        </button>
        <button className={styles.logoutBtn} onClick={handleLogout} title={t("nav:leftNav.logoutTitle")}>
          ⏻
        </button>
      </div>
    </nav>
  );
}
