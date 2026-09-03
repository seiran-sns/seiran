import { useTranslation } from "react-i18next";
import { FollowStatus } from "../../stores/userRelationshipStore";
import styles from "./UserHoverPopover.module.css";

interface UserHoverPopoverProps {
  followStatus: FollowStatus | null;
  loadingStatus: boolean;
  followActionPending: boolean;
  onToggle: (e: React.MouseEvent) => void;
}

function getFollowLabel(
  status: FollowStatus | null,
  t: (key: string) => string,
): string {
  if (status === "accepted") return t("home:noteCard.following");
  if (status === "pending") return t("home:noteCard.followPending");
  return t("home:noteCard.notFollowing");
}

/**
 * ユーザーアイコン・リンクへのマウスオーバー中に出す「フォロー状態スライドスイッチ」。
 * `NoteCard`（投稿者）・通知アイテムのユーザーリンクが共有する。
 */
export default function UserHoverPopover({
  followStatus,
  loadingStatus,
  followActionPending,
  onToggle,
}: UserHoverPopoverProps) {
  const { t } = useTranslation();
  const label = loadingStatus ? t("common:loading") : getFollowLabel(followStatus, t);

  return (
    <div className={styles.popover} onClick={(e) => e.stopPropagation()}>
      <span className={`${styles.label} ${styles[`status_${followStatus ?? "not_following"}`]}`}>
        {label}
      </span>
      <button
        type="button"
        className={`${styles.switch} ${styles[`switch_${followStatus ?? "not_following"}`]}`}
        onClick={onToggle}
        disabled={followActionPending || loadingStatus}
        title={label}
        aria-label={label}
      >
        <span className={styles.knob} />
      </button>
    </div>
  );
}
