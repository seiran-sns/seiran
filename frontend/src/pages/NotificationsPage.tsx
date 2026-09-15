import AppShell from "../components/layout/AppShell";
import NotificationsPanel from "../components/right/NotificationsPanel";
import TrendsSearchPanel from "../components/right/TrendsSearchPanel";
import { useRightPane } from "../contexts/RightPaneContext";

export default function NotificationsPage() {
  const {
    notificationsPageScrollY,
    setNotificationsPageScrollY,
    notificationsPageCache,
    setNotificationsPageCache,
  } = useRightPane();
  const center = (
    <>
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
