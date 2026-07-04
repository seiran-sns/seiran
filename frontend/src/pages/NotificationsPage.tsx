import AppShell from "../components/layout/AppShell";
import TrendsSearchPanel from "../components/right/TrendsSearchPanel";
import panel from "../components/common/Panel.module.css";

export default function NotificationsPage() {
  const center = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>通知</span>
      </header>
      <div className={panel.placeholder}>
        <span className={panel.placeholderIcon}>🔔</span>
        通知機能は準備中です。
        <br />
        リプライ・リアクション・フォローの通知エンドポイント実装後に有効化されます。
      </div>
    </>
  );

  return <AppShell center={center} right={<TrendsSearchPanel />} />;
}
