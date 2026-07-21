import { FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { ListSummary } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/ListsSettings.module.css";

interface ListsPanelProps {
  onBack: () => void;
  error: string;
  lists: ListSummary[];
  loading: boolean;
  selectedId: string | null;
  onSelect: (id: string) => void;
  newName: string;
  onNewNameChange: (value: string) => void;
  newIsPublic: boolean;
  onNewIsPublicChange: (value: boolean) => void;
  creating: boolean;
  onCreateSubmit: (e: FormEvent) => void;
}

/** ListsSettingsPage の中央ペイン：作成フォームとリスト一覧の表示。 */
export default function ListsPanel({
  onBack,
  error,
  lists,
  loading,
  selectedId,
  onSelect,
  newName,
  onNewNameChange,
  newIsPublic,
  onNewIsPublicChange,
  creating,
  onCreateSubmit,
}: ListsPanelProps) {
  const { t } = useTranslation();

  return (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={onBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("lists:listsSettingsPage.title")}</span>
      </header>

      {error && <p className={styles.error}>{error}</p>}

      <form className={styles.createForm} onSubmit={onCreateSubmit}>
        <input
          className={styles.input}
          value={newName}
          onChange={(e) => onNewNameChange(e.target.value)}
          placeholder={t("lists:listsSettingsPage.newListNamePlaceholder")}
          maxLength={100}
        />
        <label className={styles.checkboxLabel}>
          <input
            type="checkbox"
            checked={newIsPublic}
            onChange={(e) => onNewIsPublicChange(e.target.checked)}
          />
          {t("lists:listsSettingsPage.publicLabel")}
        </label>
        <button className={styles.save} type="submit" disabled={creating || !newName.trim()}>
          {creating ? t("lists:listsSettingsPage.creatingButton") : t("common:create")}
        </button>
      </form>

      {loading ? (
        <p className={panel.message}>{t("common:loading")}</p>
      ) : lists.length === 0 ? (
        <p className={panel.message}>{t("lists:listsSettingsPage.noLists")}</p>
      ) : (
        <ul className={styles.listItems}>
          {lists.map((l) => (
            <li key={l.id}>
              <button
                className={`${styles.listItemBtn} ${selectedId === l.id ? styles.listItemActive : ""}`}
                onClick={() => onSelect(l.id)}
              >
                <span className={styles.listItemName}>{l.name}</span>
                <span className={styles.listItemMeta}>
                  {l.is_public ? t("lists:listsSettingsPage.isPublicLabel") : t("lists:listsSettingsPage.isPrivateLabel")} ・{" "}
                  {t("lists:listsSettingsPage.memberCount", { count: l.member_count })}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}
