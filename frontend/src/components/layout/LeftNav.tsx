import { NavLink, useNavigate } from "react-router-dom";
import { useAuth } from "../../contexts/AuthContext";
import { useSiteMeta } from "../../contexts/SiteMetaContext";
import { isAdminRole } from "../../lib/roles";
import styles from "./AppShell.module.css";

interface NavItem {
  to: string;
  icon: string;
  label: string;
}

const NAV_ITEMS: NavItem[] = [
  { to: "/", icon: "🏠", label: "ホーム" },
  { to: "/search", icon: "🔍", label: "検索" },
  { to: "/notifications", icon: "🔔", label: "通知" },
  { to: "/settings/lists", icon: "📋", label: "リスト" },
];

export default function LeftNav({ onCompose }: { onCompose: () => void }) {
  const { user, logout } = useAuth();
  const site = useSiteMeta();
  const navigate = useNavigate();

  const navItems = isAdminRole(user?.role)
    ? [...NAV_ITEMS, { to: "/admin", icon: "🛡️", label: "管理" }]
    : NAV_ITEMS;

  function handleLogout() {
    logout();
    navigate("/login");
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
            >
              <span className={styles.navIcon}>{item.icon}</span>
              <span className={styles.navLabel}>{item.label}</span>
            </NavLink>
          </li>
        ))}
      </ul>

      <button className={styles.composeBtn} onClick={onCompose}>
        <span className={styles.navIcon}>✏️</span>
        <span className={styles.navLabel}>投稿</span>
      </button>

      <div className={styles.navFooter}>
        <button
          className={styles.userChip}
          onClick={() => user?.username && navigate(`/@${user.username}`)}
          title="自分のプロフィール"
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
        <button className={styles.logoutBtn} onClick={handleLogout} title="ログアウト">
          ⏻
        </button>
      </div>
    </nav>
  );
}
