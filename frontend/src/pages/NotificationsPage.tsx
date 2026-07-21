import { useTranslation } from "react-i18next";
import AppShell from "../components/layout/AppShell";
import NotificationsPanel from "../components/right/NotificationsPanel";
import TrendsSearchPanel from "../components/right/TrendsSearchPanel";
import panel from "../components/common/Panel.module.css";

export default function NotificationsPage() {
  const { t } = useTranslation();
  const center = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>{t("notifications:notificationsPage.title")}</span>
      </header>
      <NotificationsPanel />
    </>
  );

  return <AppShell center={center} right={<TrendsSearchPanel />} />;
}
