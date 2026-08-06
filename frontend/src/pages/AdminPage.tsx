import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Navigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import AppShell from "../components/layout/AppShell";
import Tabs from "../components/common/Tabs";
import UserManagement from "../components/admin/UserManagement";
import SiteSettingsPanel from "../components/admin/SiteSettingsPanel";
import StorageProvidersPanel from "../components/admin/StorageProvidersPanel";
import EmojisPanel from "../components/admin/EmojisPanel";
import ReportsPanel from "../components/admin/ReportsPanel";
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
const TAB_DEFS: { topic: AdminTopic; labelKey: string; Panel: () => JSX.Element }[] = [
  { topic: "users", labelKey: "admin:adminPage.tabs.users", Panel: UserManagement },
  { topic: "siteSettings", labelKey: "admin:adminPage.tabs.siteSettings", Panel: SiteSettingsPanel },
  { topic: "storage", labelKey: "admin:adminPage.tabs.storage", Panel: StorageProvidersPanel },
  { topic: "emojis", labelKey: "admin:adminPage.tabs.emojis", Panel: EmojisPanel },
  { topic: "reports", labelKey: "admin:adminPage.tabs.reports", Panel: ReportsPanel },
  { topic: "relays", labelKey: "admin:adminPage.tabs.relays", Panel: RelaysPanel },
  { topic: "authIpBlocks", labelKey: "admin:adminPage.tabs.authIpBlocks", Panel: AuthIpBlocksPanel },
];

export default function AdminPage() {
  const { t } = useTranslation();
  const { user, loading } = useAuth();
  const [tab, setTab] = useState(0);
  const headerRef = useRef<HTMLElement>(null);
  const [headerHeight, setHeaderHeight] = useState(0);

  const allowedTopics = useMemo(() => new Set(getAdminTopics(user?.role)), [user?.role]);
  const visibleTabDefs = TAB_DEFS.filter((def) => allowedTopics.has(def.topic));
  const tabs = visibleTabDefs.map((def) => t(def.labelKey));

  const handleSwipeLeft = useCallback(() => {
    setTab((prev) => (prev < tabs.length - 1 ? prev + 1 : prev));
  }, [tabs.length]);

  const handleSwipeRight = useCallback(() => {
    setTab((prev) => (prev > 0 ? prev - 1 : prev));
  }, []);

  const swipeHandlers = useSwipe({
    onSwipeLeft: handleSwipeLeft,
    onSwipeRight: handleSwipeRight,
  });

  // タブシート（Tabs）はheaderの直下にstickyで張り付ける。両者とも
  // position: sticky; top: 0 だと重なってしまうため、headerの実高さ分だけオフセットする
  // （HomePageのフィードタブと同じ手法）。
  useEffect(() => {
    const el = headerRef.current;
    if (!el) return;
    const update = () => setHeaderHeight(el.offsetHeight);
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  if (loading) return null;
  // 管理系トピックを一つも持たない役割はホームへ戻す（API 側でもトピックごとの
  // require_admin / require_emoji_admin で保護済み、#179）。
  if (!user || !canAccessAdminPage(user.role)) return <Navigate to="/" replace />;

  const ActivePanel = visibleTabDefs[tab]?.Panel;

  const center = (
    <div className={styles.swipeContainer} {...swipeHandlers}>
      <header className={panel.header} ref={headerRef}>
        <span className={panel.title}>{t("admin:adminPage.title")}</span>
      </header>
      <Tabs tabs={tabs} active={tab} onChange={setTab} sticky top={headerHeight} />
      {ActivePanel && <ActivePanel />}
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
