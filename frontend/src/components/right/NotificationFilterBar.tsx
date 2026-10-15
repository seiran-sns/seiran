import { useTranslation } from "react-i18next";
import { ActorSuggestion } from "../../api/client";
import Avatar from "../note/Avatar";
import UserIdPickerPanel from "../note/UserIdPickerPanel";
import FilterField, { FilterBar, FilterCheckList } from "../common/FilterField";
import styles from "./NotificationFilterBar.module.css";

/** 絞り込める通知種別（`NotificationItem.type` の値。Misskey 本家の種別名）。 */
export const NOTIFICATION_TYPES = [
  "reaction",
  "mention",
  "reply",
  "renote",
  "quote",
  "follow",
  "receiveFollowRequest",
  "followRequestAccepted",
  "moveRefollowed",
  "moveAlreadyFollowing",
] as const;

export interface NotificationFilterState {
  types: string[];
  users: ActorSuggestion[];
}

export const EMPTY_NOTIFICATION_FILTER: NotificationFilterState = { types: [], users: [] };

/** API へ渡す絞り込み条件。未指定（全種別を選択した場合を含む）は `undefined`。 */
export function toNotificationQuery(filter: NotificationFilterState): {
  includeTypes?: string[];
  userIds?: string[];
} {
  const allTypes = filter.types.length === 0 || filter.types.length >= NOTIFICATION_TYPES.length;
  return {
    includeTypes: allTypes ? undefined : filter.types,
    userIds: filter.users.length > 0 ? filter.users.map((u) => u.actor_id) : undefined,
  };
}

interface NotificationFilterBarProps {
  filter: NotificationFilterState;
  onChange: (filter: NotificationFilterState) => void;
}

/** 通知一覧の最上部に置く絞り込み欄（通知種別・ユーザー指定）。両方未指定が初期状態で、
 * 指定した条件は AND で作用する。 */
export default function NotificationFilterBar({ filter, onChange }: NotificationFilterBarProps) {
  const { t } = useTranslation();
  const typeLabel = (type: string) => t(`notifications:filter.types.${type}`);

  const toggleType = (type: string) =>
    onChange({
      ...filter,
      types: filter.types.includes(type) ? filter.types.filter((x) => x !== type) : [...filter.types, type],
    });

  const addUser = (actor: ActorSuggestion) => {
    if (filter.users.some((u) => u.actor_id === actor.actor_id)) return;
    onChange({ ...filter, users: [...filter.users, actor] });
  };

  const removeUser = (actorId: string) =>
    onChange({ ...filter, users: filter.users.filter((u) => u.actor_id !== actorId) });

  return (
    <FilterBar>
      <FilterField
        label={t("notifications:filter.typeLabel")}
        placeholder={t("notifications:filter.typePlaceholder")}
        tags={filter.types.map(typeLabel)}
        onClear={() => onChange({ ...filter, types: [] })}
      >
        <FilterCheckList
          options={NOTIFICATION_TYPES.map((type) => ({ value: type, label: typeLabel(type) }))}
          selected={filter.types}
          onToggle={toggleType}
        />
      </FilterField>
      <FilterField
        label={t("notifications:filter.userLabel")}
        placeholder={t("notifications:filter.userPlaceholder")}
        tags={filter.users.map((u) => u.display_name || u.username)}
        onClear={() => onChange({ ...filter, users: [] })}
      >
        <UserIdPickerPanel onPick={addUser} />
        {filter.users.length > 0 && (
          <ul className={styles.selectedUsers}>
            {filter.users.map((u) => (
              <li key={u.actor_id} className={styles.selectedUser}>
                <Avatar url={u.avatar_url} name={u.display_name || u.username} size={24} />
                <span className={styles.selectedName}>
                  {u.display_name || u.username}
                  <span className={styles.selectedHandle}>
                    @{u.username}
                    {u.domain ? `@${u.domain}` : ""}
                  </span>
                </span>
                <button type="button" className={styles.removeBtn} onClick={() => removeUser(u.actor_id)}>
                  {t("common:delete")}
                </button>
              </li>
            ))}
          </ul>
        )}
      </FilterField>
    </FilterBar>
  );
}
