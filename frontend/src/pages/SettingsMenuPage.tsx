import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import AppShell from "../components/layout/AppShell";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import panel from "../components/common/Panel.module.css";
import styles from "./SettingsMenu.module.css";

interface SettingsMenuItem {
  to?: string;
  icon: string;
  labelKey: string;
  /** バックエンド未実装で近日公開扱いの項目（#55: アプリトークン）。 */
  disabled?: boolean;
}

const ITEMS: SettingsMenuItem[] = [
  { to: "/settings/account", icon: "🔐", labelKey: "menu.account" },
  { to: "/settings/profile", icon: "🪪", labelKey: "menu.profile" },
  { to: "/settings/mutes-blocks", icon: "🚫", labelKey: "menu.mutesBlocks" },
  { to: "/settings/lists", icon: "📋", labelKey: "menu.lists" },
  { icon: "🔑", labelKey: "menu.appTokens", disabled: true },
];

/** メインメニューの「設定」から遷移する設定項目一覧（#55）。 */
export default function SettingsMenuPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const goBack = useGoBack();

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("account:menu.title")}</span>
      </header>
      <ul className={styles.list}>
        {ITEMS.map((item) => (
          <li key={item.labelKey}>
            <button
              type="button"
              className={styles.row}
              disabled={item.disabled}
              onClick={() => item.to && navigate(item.to)}
            >
              <span className={styles.icon}>{item.icon}</span>
              <span className={styles.label}>{t(`account:${item.labelKey}`)}</span>
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
