import { useCallback, useMemo } from "react";
import { Navigate, useNavigate, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import AppShell from "../components/layout/AppShell";
import Tabs from "../components/common/Tabs";
import UserManagement from "../components/admin/UserManagement";
import SiteSettingsPanel from "../components/admin/SiteSettingsPanel";
import StorageProvidersPanel from "../components/admin/StorageProvidersPanel";
import EmojisPanel from "../components/admin/EmojisPanel";
import ReportsPanel from "../components/admin/ReportsPanel";
import SuspendedActorsPanel from "../components/admin/SuspendedActorsPanel";
import RelaysPanel from "../components/admin/RelaysPanel";
import AuthIpBlocksPanel from "../components/admin/AuthIpBlocksPanel";
import { useAuth } from "../contexts/AuthContext";
import { AdminTopic, canAccessAdminPage, getAdminTopics } from "../lib/roles";
import { useSwipe } from "../hooks/useSwipe";
import panel from "../components/common/Panel.module.css";
import styles from "./Admin.module.css";
import TwemojiEmoji from "../components/common/TwemojiEmoji";

// タブの並び順・内容。#179: ロールが持つトピック権限（getAdminTopics）に応じて
// 表示するタブを絞り込む（権限のないトピックはタブごと非表示になる）。
// `slug`は`/admin/:slug`のURL片（リロード・リンク共有で同じタブを開くため）。
const TAB_DEFS: { topic: AdminTopic; slug: string; labelKey: string; Panel: () => JSX.Element }[] = [
  { topic: "users", slug: "users", labelKey: "admin:adminPage.tabs.users", Panel: UserManagement },
  { topic: "siteSettings", slug: "site-settings", labelKey: "admin:adminPage.tabs.siteSettings", Panel: SiteSettingsPanel },
  { topic: "storage", slug: "storage", labelKey: "admin:adminPage.tabs.storage", Panel: StorageProvidersPanel },
  { topic: "emojis", slug: "emojis", labelKey: "admin:adminPage.tabs.emojis", Panel: EmojisPanel },
  { topic: "reports", slug: "reports", labelKey: "admin:adminPage.tabs.reports", Panel: ReportsPanel },
  { topic: "suspendedActors", slug: "suspended-actors", labelKey: "admin:adminPage.tabs.suspendedActors", Panel: SuspendedActorsPanel },
  { topic: "relays", slug: "relays", labelKey: "admin:adminPage.tabs.relays", Panel: RelaysPanel },
  { topic: "authIpBlocks", slug: "auth-ip-blocks", labelKey: "admin:adminPage.tabs.authIpBlocks", Panel: AuthIpBlocksPanel },
];

export default function AdminPage() {
  const { t } = useTranslation();
  const { user, loading } = useAuth();
  const navigate = useNavigate();
  const { slug } = useParams<{ slug: string }>();

  const allowedTopics = useMemo(() => new Set(getAdminTopics(user?.role)), [user?.role]);
  const visibleTabDefs = useMemo(() => TAB_DEFS.filter((def) => allowedTopics.has(def.topic)), [allowedTopics]);
  const tabs = visibleTabDefs.map((def) => t(def.labelKey));

  const tab = visibleTabDefs.findIndex((def) => def.slug === slug);

  const goToTab = useCallback(
    (index: number) => {
      const def = visibleTabDefs[index];
      if (def) navigate(`/admin/${def.slug}`);
    },
    [visibleTabDefs, navigate],
  );

  const handleSwipeLeft = useCallback(() => {
    if (tab >= 0 && tab < tabs.length - 1) goToTab(tab + 1);
  }, [tab, tabs.length, goToTab]);

  const handleSwipeRight = useCallback(() => {
    if (tab > 0) goToTab(tab - 1);
  }, [tab, goToTab]);

  const swipeHandlers = useSwipe({
    onSwipeLeft: handleSwipeLeft,
    onSwipeRight: handleSwipeRight,
  });

  if (loading) return null;
  // 管理系トピックを一つも持たない役割はホームへ戻す（API 側でもトピックごとの
  // require_admin / require_emoji_admin で保護済み、#179）。
  if (!user || !canAccessAdminPage(user.role)) return <Navigate to="/" replace />;

  // `/admin`・未知のslug・権限のないタブは、先頭の表示可能なタブへ寄せる。
  if (tab < 0) return <Navigate to={`/admin/${visibleTabDefs[0].slug}`} replace />;

  const ActivePanel = visibleTabDefs[tab].Panel;

  const center = (
    <div className={styles.swipeContainer} {...swipeHandlers}>
      <Tabs tabs={tabs} active={tab} onChange={goToTab} sticky top={0} scrollable />
      <ActivePanel />
    </div>
  );

  const right = (
    <div className={panel.placeholder}>
      <TwemojiEmoji emoji="🛡️" className={panel.placeholderIcon} />
      {t("admin:adminPage.rightPanelDescription")}
    </div>
  );

  return <AppShell center={center} right={right} />;
}
