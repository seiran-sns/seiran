import { FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { ActorSuggestion, ListDetail } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/ListsSettings.module.css";

interface ListDetailPanelProps {
  selectedId: string | null;
  detail: ListDetail | null;
  detailLoading: boolean;

  editName: string;
  onEditNameChange: (value: string) => void;
  editIsPublic: boolean;
  onEditIsPublicChange: (value: boolean) => void;
  saving: boolean;
  onSaveEdit: (e: FormEvent) => void;
  onDelete: (id: string) => void;

  memberTarget: string;
  onMemberTargetChange: (value: string) => void;
  addingMember: boolean;
  memberError: string;
  onAddMember: (e: FormEvent) => void;
  onRemoveMember: (actorId: string) => void;

  suggestions: ActorSuggestion[];
  showSuggestions: boolean;
  onShowSuggestions: (value: boolean) => void;
  onSelectSuggestion: (s: ActorSuggestion) => void;
}

/** ListsSettingsPage の右（または狭幅時は中央連続表示の）ペイン：選択中リストの改名/公開設定/削除とメンバー編集。 */
export default function ListDetailPanel({
  selectedId,
  detail,
  detailLoading,
  editName,
  onEditNameChange,
  editIsPublic,
  onEditIsPublicChange,
  saving,
  onSaveEdit,
  onDelete,
  memberTarget,
  onMemberTargetChange,
  addingMember,
  memberError,
  onAddMember,
  onRemoveMember,
  suggestions,
  showSuggestions,
  onShowSuggestions,
  onSelectSuggestion,
}: ListDetailPanelProps) {
  const { t } = useTranslation();

  if (!selectedId) {
    return <p className={panel.message}>{t("lists:listsSettingsPage.selectListPrompt")}</p>;
  }
  if (detailLoading || !detail) {
    return <p className={panel.message}>{t("common:loading")}</p>;
  }

  return (
    <>
      <div className={panel.rightHeader}>
        {t("lists:listsSettingsPage.editingHeader", { name: detail.name })}
      </div>

      <form className={styles.editForm} onSubmit={onSaveEdit}>
        <input
          className={styles.input}
          value={editName}
          onChange={(e) => onEditNameChange(e.target.value)}
          maxLength={100}
        />
        <label className={styles.checkboxLabel}>
          <input
            type="checkbox"
            checked={editIsPublic}
            onChange={(e) => onEditIsPublicChange(e.target.checked)}
          />
          {t("lists:listsSettingsPage.publicLabel")}
        </label>
        <button className={styles.save} type="submit" disabled={saving || !editName.trim()}>
          {saving ? t("common:saving") : t("common:save")}
        </button>
        <button type="button" className={styles.dangerBtn} onClick={() => onDelete(detail.id)}>
          {t("common:delete")}
        </button>
      </form>

      <form className={styles.memberForm} onSubmit={onAddMember}>
        <div className={styles.memberInputWrap}>
          <input
            className={styles.input}
            value={memberTarget}
            onChange={(e) => {
              onMemberTargetChange(e.target.value);
              onShowSuggestions(true);
            }}
            onFocus={() => onShowSuggestions(true)}
            placeholder={t("lists:listsSettingsPage.memberSearchPlaceholder")}
            autoComplete="off"
          />
          {showSuggestions && suggestions.length > 0 && (
            <ul className={styles.suggestList}>
              {suggestions.map((s) => (
                <li key={s.actor_id}>
                  <button
                    type="button"
                    className={styles.suggestItem}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => onSelectSuggestion(s)}
                  >
                    <span className={styles.suggestAvatar}>
                      {s.avatar_url ? (
                        <img src={s.avatar_url} alt="" />
                      ) : (
                        <span>{(s.display_name || s.username)[0]?.toUpperCase()}</span>
                      )}
                    </span>
                    <span className={styles.suggestName}>
                      {s.display_name || s.username}
                      <span className={styles.suggestHandle}>
                        @{s.username}
                        {s.domain ? `@${s.domain}` : ""}
                      </span>
                    </span>
                    <span className={styles.suggestType}>{s.actor_type}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <button className={styles.save} type="submit" disabled={addingMember || !memberTarget.trim()}>
          {addingMember ? t("lists:listsSettingsPage.addingMemberButton") : t("lists:listsSettingsPage.addMemberButton")}
        </button>
      </form>
      {memberError && <p className={styles.error}>{memberError}</p>}

      <ul className={styles.memberList}>
        {detail.members.map((m) => (
          <li key={m.actor_id} className={styles.memberRow}>
            <span className={styles.memberAvatar}>
              {m.avatar_url ? <img src={m.avatar_url} alt="" /> : <span>{(m.display_name || m.username)[0]?.toUpperCase()}</span>}
            </span>
            <span className={styles.memberName}>
              {m.display_name || m.username}
              <span className={styles.memberHandle}>
                @{m.username}
                {m.domain ? `@${m.domain}` : ""}
              </span>
            </span>
            <span className={styles.memberType}>{m.actor_type}</span>
            <button className={styles.removeBtn} onClick={() => onRemoveMember(m.actor_id)}>
              {t("common:delete")}
            </button>
          </li>
        ))}
        {detail.members.length === 0 && <p className={panel.message}>{t("lists:listsSettingsPage.noMembers")}</p>}
      </ul>
    </>
  );
}
