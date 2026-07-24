import { useCallback, useEffect, useRef, useState } from "react";
import { Navigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import AppShell from "../components/layout/AppShell";
import Tabs from "../components/common/Tabs";
import UserManagement from "../components/admin/UserManagement";
import SiteSettingsPanel from "../components/admin/SiteSettingsPanel";
import StorageProvidersPanel from "../components/admin/StorageProvidersPanel";
import EmojisPanel from "../components/admin/EmojisPanel";
import { useAuth } from "../contexts/AuthContext";
import { isAdminRole } from "../lib/roles";
import { useSwipe } from "../hooks/useSwipe";
import panel from "../components/common/Panel.module.css";
import styles from "./Admin.module.css";

export default function AdminPage() {
  const { t } = useTranslation();
  const { user, loading } = useAuth();
  const [tab, setTab] = useState(0);
  const headerRef = useRef<HTMLElement>(null);
  const [headerHeight, setHeaderHeight] = useState(0);

  const tabs = [
    t("admin:adminPage.tabs.users"),
    t("admin:adminPage.tabs.siteSettings"),
    t("admin:adminPage.tabs.storage"),
    t("admin:adminPage.tabs.emojis"),
  ];

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
  // 管理者・モデレーター以外はホームへ戻す（API 側でも require_admin で保護済み）。
  if (!user || !isAdminRole(user.role)) return <Navigate to="/" replace />;

  const center = (
    <div className={styles.swipeContainer} {...swipeHandlers}>
      <header className={panel.header} ref={headerRef}>
        <span className={panel.title}>{t("admin:adminPage.title")}</span>
      </header>
      <Tabs tabs={tabs} active={tab} onChange={setTab} sticky top={headerHeight} />
      {tab === 0 && <UserManagement />}
      {tab === 1 && <SiteSettingsPanel />}
      {tab === 2 && <StorageProvidersPanel />}
      {tab === 3 && <EmojisPanel />}
    </div>
  );

  const right = (
    <div className={panel.placeholder}>
      <span className={panel.placeholderIcon}>🛡️</span>
      {t("admin:adminPage.rightPanelDescription")}
    </div>
  );

  return <AppShell center={center} right={right} />;
}
