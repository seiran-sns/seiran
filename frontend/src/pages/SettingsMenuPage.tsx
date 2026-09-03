import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api } from "../api/client";
import AppShell from "../components/layout/AppShell";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import { useStreamingContext } from "../contexts/StreamingContext";
import panel from "../components/common/Panel.module.css";
import TwemojiEmoji from "../components/common/TwemojiEmoji";
import styles from "./SettingsMenu.module.css";

interface SettingsMenuItem {
  to?: string;
  icon: string;
  labelKey: string;
  disabled?: boolean;
  badge?: number;
}

const BASE_ITEMS: SettingsMenuItem[] = [
  { to: "/settings/account", icon: "🔐", labelKey: "menu.account" },
  { to: "/settings/profile", icon: "🪪", labelKey: "menu.profile" },
  { to: "/settings/mutes-blocks", icon: "🚫", labelKey: "menu.mutesBlocks" },
  { to: "/settings/privacy", icon: "🔒", labelKey: "menu.privacy" },
  { to: "/settings/lists", icon: "📋", labelKey: "menu.lists" },
  { to: "/settings/appearance", icon: "🎨", labelKey: "menu.appearance" },
  { to: "/settings/app-tokens", icon: "🔑", labelKey: "menu.appTokens" },
  { to: "/settings/follow-import", icon: "🚚", labelKey: "menu.followImport" },
];

/** メインメニューの「設定」から遷移する設定項目一覧（#55）。 */
export default function SettingsMenuPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const goBack = useGoBack();
  const { followRequestCount } = useStreamingContext();
  const [isLocked, setIsLocked] = useState(false);

  useEffect(() => {
    api.account.getLock().then((r) => setIsLocked(r.is_locked)).catch(() => {});
  }, []);

  // 承認制フォロー（鍵アカウント）中のみ「承認待ちフォロー」項目を表示する。
  const items: SettingsMenuItem[] = isLocked
    ? [
        ...BASE_ITEMS,
        {
          to: "/settings/follow-requests",
          icon: "👥",
          labelKey: "menu.followRequests",
          badge: followRequestCount,
        },
      ]
    : BASE_ITEMS;

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("account:menu.title")}</span>
      </header>
      <ul className={styles.list}>
        {items.map((item) => (
          <li key={item.labelKey}>
            <button
              type="button"
              className={styles.row}
              disabled={item.disabled}
              onClick={() => item.to && navigate(item.to)}
            >
              <TwemojiEmoji emoji={item.icon} className={styles.icon} />
              <span className={styles.label}>{t(`account:${item.labelKey}`)}</span>
              {!!item.badge && <span className={styles.badge}>{item.badge > 99 ? "99+" : item.badge}</span>}
              {item.disabled ? (
                <span className={styles.comingSoon}>{t("account:menu.comingSoon")}</span>
              ) : (
                <span className={styles.chevron}>›</span>
              )}
            </button>
          </li>
        ))}
      </ul>
    </>
  );

  return <AppShell center={center} />;
}
