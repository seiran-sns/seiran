import { ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ActorSuggestion } from "../../api/client";
import Avatar from "../note/Avatar";
import UserIdPickerPanel from "../note/UserIdPickerPanel";
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

interface FilterFieldProps {
  label: string;
  placeholder: string;
  tags: string[];
  /** 指定を全部外す。タグがあるときだけ✕ボタンを出す。 */
  onClear: () => void;
  children: ReactNode;
}

/** テキストボックス風の表示欄。クリックで下にポップオーバー（選択UI）を開き、選択内容はタグ状に並べる。
 * はみ出した分は右端の「…」で省略する。 */
function FilterField({ label, placeholder, tags, onClear, children }: FilterFieldProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const tagsRef = useRef<HTMLSpanElement>(null);
  const [overflowing, setOverflowing] = useState(false);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  useLayoutEffect(() => {
    const el = tagsRef.current;
    if (!el) return;
    const measure = () => setOverflowing(el.scrollWidth > el.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [tags]);

  return (
    <div className={styles.field} ref={wrapRef}>
      <button
        type="button"
        className={styles.box}
        aria-label={label}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        {tags.length === 0 ? (
          <span className={styles.placeholder}>{placeholder}</span>
        ) : (
          <>
            <span className={styles.tags} ref={tagsRef}>
              {tags.map((tag) => (
                <span key={tag} className={styles.tag}>
                  {tag}
                </span>
              ))}
            </span>
            {overflowing && <span className={styles.ellipsis}>…</span>}
          </>
        )}
      </button>
      {tags.length > 0 && (
        <button type="button" className={styles.clear} aria-label={t("notifications:filter.clear")} onClick={onClear}>
          ✕
        </button>
      )}
      {open && <div className={styles.popover}>{children}</div>}
    </div>
  );
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
    <div className={styles.bar}>
      <FilterField
        label={t("notifications:filter.typeLabel")}
        placeholder={t("notifications:filter.typePlaceholder")}
        tags={filter.types.map(typeLabel)}
        onClear={() => onChange({ ...filter, types: [] })}
      >
        <ul className={styles.typeList}>
          {NOTIFICATION_TYPES.map((type) => (
            <li key={type}>
              <label className={styles.typeItem}>
                <input type="checkbox" checked={filter.types.includes(type)} onChange={() => toggleType(type)} />
                {typeLabel(type)}
              </label>
            </li>
          ))}
        </ul>
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
    </div>
  );
}
