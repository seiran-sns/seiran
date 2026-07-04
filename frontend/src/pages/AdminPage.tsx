import { useState } from "react";
import { Navigate } from "react-router-dom";
import AppShell from "../components/layout/AppShell";
import Tabs from "../components/common/Tabs";
import UserManagement from "../components/admin/UserManagement";
import SiteSettingsPanel from "../components/admin/SiteSettingsPanel";
import StorageProvidersPanel from "../components/admin/StorageProvidersPanel";
import EmojisPanel from "../components/admin/EmojisPanel";
import { useAuth } from "../contexts/AuthContext";
import { isAdminRole } from "../lib/roles";
import panel from "../components/common/Panel.module.css";

const TABS = ["ユーザー", "サイト設定", "ストレージ", "絵文字"];

export default function AdminPage() {
  const { user, loading } = useAuth();
  const [tab, setTab] = useState(0);

  if (loading) return null;
  // 管理者・モデレーター以外はホームへ戻す（API 側でも require_admin で保護済み）。
  if (!user || !isAdminRole(user.role)) return <Navigate to="/" replace />;

  const center = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>管理</span>
      </header>
      <Tabs tabs={TABS} active={tab} onChange={setTab} />
      {tab === 0 && <UserManagement />}
      {tab === 1 && <SiteSettingsPanel />}
      {tab === 2 && <StorageProvidersPanel />}
      {tab === 3 && <EmojisPanel />}
    </>
  );

  const right = (
    <div className={panel.placeholder}>
      <span className={panel.placeholderIcon}>🛡️</span>
      管理操作はこの画面から行えます。各操作はサーバー側でも管理者権限を検証します。
    </div>
  );

  return <AppShell center={center} right={right} />;
}
