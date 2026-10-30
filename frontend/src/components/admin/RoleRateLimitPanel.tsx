import { FormEvent, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage } from "../../api/client";
import type { SiteSettings } from "../../api/types";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";
import sheet from "./RoleRateLimitPanel.module.css";

/** 横軸に並べるロール（権限の弱い順）。 */
const ROLE_COLUMNS = ["quarantine", "silence", "user", "emoji-editor", "moderator", "admin"] as const;
type RoleColumn = (typeof ROLE_COLUMNS)[number];

/** 独自の設定値を持てるロール。それ以外（admin を除く）は user の値がそのまま適用される。 */
type OwnLimitRole = "user" | "moderator" | "quarantine";

type RateLimitKey = Extract<
  keyof SiteSettings,
  `${string}_rate_limit_${string}` | `list_max_count_${string}` | `list_member_max_${string}`
>;

interface LimitRow {
  id: "post" | "follow" | "listCount" | "listMember" | "search";
  window?: { key: RateLimitKey; unit: "minutesUnit" | "hoursUnit" };
  keys: Partial<Record<OwnLimitRole, RateLimitKey>> & { user: RateLimitKey };
}

// バックエンドの`rate_limit::role_limit`・`check_follow_rate_limit`と対応する。
const LIMIT_ROWS: LimitRow[] = [
  {
    id: "post",
    window: { key: "post_rate_limit_window_minutes", unit: "minutesUnit" },
    keys: { user: "post_rate_limit_max_user", moderator: "post_rate_limit_max_moderator" },
  },
  {
    id: "follow",
    window: { key: "follow_rate_limit_window_hours", unit: "hoursUnit" },
    keys: {
      user: "follow_rate_limit_max_user",
      moderator: "follow_rate_limit_max_moderator",
      quarantine: "follow_rate_limit_max_quarantine",
    },
  },
  { id: "listCount", keys: { user: "list_max_count_user", moderator: "list_max_count_moderator" } },
  { id: "listMember", keys: { user: "list_member_max_user", moderator: "list_member_max_moderator" } },
  {
    id: "search",
    window: { key: "search_rate_limit_window_minutes", unit: "minutesUnit" },
    keys: { user: "search_rate_limit_max_user", moderator: "search_rate_limit_max_moderator" },
  },
];

const ALL_KEYS: RateLimitKey[] = LIMIT_ROWS.flatMap((row) => [
  ...(row.window ? [row.window.key] : []),
  ...Object.values(row.keys),
]);

/** ロール別レート制限を、横軸ロール・縦軸制限項目のマトリクスで編集する。 */
export default function RoleRateLimitPanel() {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [values, setValues] = useState<Partial<Record<RateLimitKey, string>>>({});

  useEffect(() => {
    api.admin
      .getSiteSettings()
      .then((s) => setValues(Object.fromEntries(ALL_KEYS.map((k) => [k, s[k]]))))
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }, []);

  function setValue(key: RateLimitKey, value: string) {
    setValues((prev) => ({ ...prev, [key]: value }));
    setSaved(false);
  }

  async function save(e: FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      await api.admin.updateSiteSettings(values);
      setSaved(true);
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  function renderCell(row: LimitRow, role: RoleColumn) {
    if (role === "admin") {
      return <span className={sheet.unlimited}>{t("admin:roleRateLimitPanel.unlimited")}</span>;
    }
    const ownKey = role === "silence" || role === "emoji-editor" ? undefined : row.keys[role];
    if (!ownKey) {
      return (
        <span className={sheet.inherited} title={t("admin:roleRateLimitPanel.inheritedFromUser")}>
          {values[row.keys.user]}
        </span>
      );
    }
    return (
      <input
        className={`${styles.input} ${sheet.cellInput}`}
        type="number"
        min={1}
        aria-label={`${t(`admin:roleRateLimitPanel.rows.${row.id}`)} (${role})`}
        value={values[ownKey] ?? ""}
        onChange={(e) => setValue(ownKey, e.target.value)}
      />
    );
  }

  if (loading) return <p className={panel.message}>{t("common:loading")}</p>;

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:roleRateLimitPanel.title")}</h2>
      <form onSubmit={save}>
        <div className={styles.card}>
          <p className={styles.hint}>{t("admin:roleRateLimitPanel.hint")}</p>
          <div className={sheet.scroller}>
            <table className={sheet.sheet}>
              <thead>
                <tr>
                  <th className={sheet.itemHeader}>{t("admin:roleRateLimitPanel.itemHeader")}</th>
                  <th className={sheet.windowHeader}>{t("admin:roleRateLimitPanel.windowHeader")}</th>
                  {ROLE_COLUMNS.map((role) => (
                    <th key={role} className={sheet.roleHeader} scope="col">
                      <span className={sheet.vertical}>{role}</span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {LIMIT_ROWS.map((row) => (
                  <tr key={row.id}>
                    <th scope="row" className={sheet.itemCell}>
                      {t(`admin:roleRateLimitPanel.rows.${row.id}`)}
                    </th>
                    <td className={sheet.windowCell}>
                      {row.window ? (
                        <span className={sheet.windowInput}>
                          <input
                            className={`${styles.input} ${sheet.cellInput}`}
                            type="number"
                            min={1}
                            aria-label={`${t(`admin:roleRateLimitPanel.rows.${row.id}`)} (${t("admin:roleRateLimitPanel.windowHeader")})`}
                            value={values[row.window.key] ?? ""}
                            onChange={(e) => setValue(row.window!.key, e.target.value)}
                          />
                          {t(`admin:roleRateLimitPanel.${row.window.unit}`)}
                        </span>
                      ) : (
                        <span className={sheet.inherited}>{t("admin:roleRateLimitPanel.noWindow")}</span>
                      )}
                    </td>
                    {ROLE_COLUMNS.map((role) => (
                      <td key={role} className={sheet.cell}>
                        {renderCell(row, role)}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
        <div className={styles.stickySaveBar}>
          {error && <p className={styles.error}>{error}</p>}
          {saved && <p className={styles.success}>{t("admin:roleRateLimitPanel.savedMessage")}</p>}
          <button className={styles.btn} type="submit" disabled={saving}>
            {saving ? t("common:saving") : t("common:save")}
          </button>
        </div>
      </form>
    </div>
  );
}
