import { useTranslation } from "react-i18next";
import AppShell from "../components/layout/AppShell";
import NotificationsPanel from "../components/right/NotificationsPanel";
import TrendsSearchPanel from "../components/right/TrendsSearchPanel";
import { useRightPane } from "../contexts/RightPaneContext";
import panel from "../components/common/Panel.module.css";

export default function NotificationsPage() {
  const { t } = useTranslation();
  const {
    notificationsPageScrollY,
    setNotificationsPageScrollY,
    notificationsPageCache,
    setNotificationsPageCache,
  } = useRightPane();
  const center = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>{t("notifications:notificationsPage.title")}</span>
      </header>
      <NotificationsPanel
        scrollY={notificationsPageScrollY}
        onScrollYChange={setNotificationsPageScrollY}
        cache={notificationsPageCache}
        onCacheChange={setNotificationsPageCache}
      />
    </>
  );

  return <AppShell center={center} right={<TrendsSearchPanel />} />;
}
