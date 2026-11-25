import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useStreamingContext } from "../contexts/StreamingContext";
import UiIcon from "../components/common/UiIcon";
import styles from "./SettingsMenu.module.css";

interface FeaturesMenuItem {
  to: string;
  icon: string;
  labelKey: string;
  badge?: number;
  /** 件数バッジとは別の「予約投稿がある」印（🕒）。件数バッジがあればそちらを優先して出す。 */
  clock?: boolean;
}

/** メインメニューの「機能」から遷移する、対応や確認の要る項目の一覧（予約投稿・承認待ちフォロー）。 */
export default function FeaturesMenuPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { followRequestCount, scheduledPostCounts } = useStreamingContext();
  const [isLocked, setIsLocked] = useState(false);

  useEffect(() => {
    api.account.getLock().then((r) => setIsLocked(r.is_locked)).catch(() => {});
  }, []);

  const items: FeaturesMenuItem[] = [
    {
      to: "/features/scheduled-posts",
      icon: "🕒",
      labelKey: "menu.scheduledPosts",
      badge: scheduledPostCounts.failed,
      clock: scheduledPostCounts.pending > 0,
    },
    // 承認制フォロー（鍵アカウント）中だけ出す。承認制をやめた後に残った承認待ちも処理できるよう、
    // 承認待ちがあれば出す。
    ...(isLocked || followRequestCount > 0
      ? [
          {
            to: "/features/follow-requests",
            icon: "👥",
            labelKey: "menu.followRequests",
            badge: followRequestCount,
          },
        ]
      : []),
  ];

  const center = (
    <ul className={styles.list}>
      {items.map((item) => (
        <li key={item.labelKey}>
          <button type="button" className={styles.row} onClick={() => navigate(item.to)}>
            <UiIcon emoji={item.icon} className={styles.icon} />
            <span className={styles.label}>{t(`features:${item.labelKey}`)}</span>
            {!!item.badge && <span className={styles.badge}>{item.badge > 99 ? "99+" : item.badge}</span>}
            {!item.badge && item.clock && (
              <span className={styles.clock} title={t("features:menu.scheduledPostsPending")}>
                <UiIcon emoji="🕒" />
              </span>
            )}
            <span className={styles.chevron}>›</span>
          </button>
        </li>
      ))}
    </ul>
  );

  return <AppShell center={center} />;
}
