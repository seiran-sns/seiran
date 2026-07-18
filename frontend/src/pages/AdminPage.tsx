import { useState } from "react";
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
import panel from "../components/common/Panel.module.css";

export default function AdminPage() {
  const { t } = useTranslation();
  const { user, loading } = useAuth();
  const [tab, setTab] = useState(0);

  if (loading) return null;
  // 管理者・モデレーター以外はホームへ戻す（API 側でも require_admin で保護済み）。
  if (!user || !isAdminRole(user.role)) return <Navigate to="/" replace />;

  const tabs = [
    t("admin:adminPage.tabs.users"),
    t("admin:adminPage.tabs.siteSettings"),
    t("admin:adminPage.tabs.storage"),
    t("admin:adminPage.tabs.emojis"),
  ];

  const center = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>{t("admin:adminPage.title")}</span>
      </header>
      <Tabs tabs={tabs} active={tab} onChange={setTab} />
      {tab === 0 && <UserManagement />}
      {tab === 1 && <SiteSettingsPanel />}
      {tab === 2 && <StorageProvidersPanel />}
      {tab === 3 && <EmojisPanel />}
    </>
  );

  const right = (
    <div className={panel.placeholder}>
      <span className={panel.placeholderIcon}>🛡️</span>
      {t("admin:adminPage.rightPanelDescription")}
    </div>
  );

  return <AppShell center={center} right={right} />;
}
