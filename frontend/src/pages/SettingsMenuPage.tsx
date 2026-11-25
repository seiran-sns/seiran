import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import AppShell from "../components/layout/AppShell";
import UiIcon from "../components/common/UiIcon";
import { useMigrationImportActive } from "../hooks/useMigrationImport";
import styles from "./SettingsMenu.module.css";
import { useBackgroundImports } from "../contexts/BackgroundImportsContext";

interface SettingsMenuItem {
  to?: string;
  icon: string;
  labelKey: string;
  disabled?: boolean;
  badge?: number;
  /** 件数の無い「続いている処理がある」印（明滅する赤丸）。 */
  dot?: boolean;
}

const BASE_ITEMS: SettingsMenuItem[] = [
  { to: "/settings/account", icon: "🔐", labelKey: "menu.account" },
  { to: "/settings/profile", icon: "🪪", labelKey: "menu.profile" },
  { to: "/settings/mutes-blocks", icon: "🚫", labelKey: "menu.mutesBlocks" },
  { to: "/settings/privacy", icon: "🔒", labelKey: "menu.privacy" },
  { to: "/settings/guides", icon: "🔕", labelKey: "menu.guides" },
  { to: "/settings/lists", icon: "📋", labelKey: "menu.lists" },
  { to: "/settings/appearance", icon: "🎨", labelKey: "menu.appearance" },
  { to: "/settings/app-tokens", icon: "🔑", labelKey: "menu.appTokens" },
  { to: "/settings/follow-import", icon: "🚚", labelKey: "menu.followImport" },
];

/** メインメニューの「設定」から遷移する設定項目一覧（#55）。 */
export default function SettingsMenuPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const migrationImportActive = useMigrationImportActive();
  const { followImportActive } = useBackgroundImports();

  // Bluesky からの転入の取り込みが裏で続いている間は、インポートの項目に印を出す。
  // 承認待ちフォローは「機能」メニュー（`FeaturesMenuPage`）にある。
  const items: SettingsMenuItem[] = BASE_ITEMS.map((item) =>
    item.to === "/settings/follow-import" ? { ...item, dot: migrationImportActive || followImportActive } : item,
  );

  const center = (
    <>
      <ul className={styles.list}>
        {items.map((item) => (
          <li key={item.labelKey}>
            <button
              type="button"
              className={styles.row}
              disabled={item.disabled}
              onClick={() => item.to && navigate(item.to)}
            >
              <UiIcon emoji={item.icon} className={styles.icon} />
              <span className={styles.label}>{t(`account:${item.labelKey}`)}</span>
              {!!item.badge && <span className={styles.badge}>{item.badge > 99 ? "99+" : item.badge}</span>}
              {!item.badge && item.dot && <span className={styles.dot} aria-hidden="true" />}
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
