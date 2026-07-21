import { useTranslation } from "react-i18next";
import { ReactionSummary } from "../../api/client";
import ReactionChips from "./ReactionChips";
import ReactionPicker from "./ReactionPicker";
import styles from "./NoteCard.module.css";

interface NoteCardActionsProps {
  reactions: ReactionSummary[];
  reactionPending: boolean;
  onToggleReaction: (emoji: string) => void;
  onReply: (e: React.MouseEvent) => void;
  reposted: boolean;
  reposting: boolean;
  unreposting: boolean;
  isPrivateRepostTarget: boolean;
  onRepost: (e: React.MouseEvent) => void;
  isSelf: boolean;
  pinned: boolean;
  pinning: boolean;
  onTogglePin: (e: React.MouseEvent) => void;
}

/** 投稿カード下部のリアクションチップ＋アクションボタン群（返信/リポスト/リアクション/ピン留め）。 */
export default function NoteCardActions({
  reactions,
  reactionPending,
  onToggleReaction,
  onReply,
  reposted,
  reposting,
  unreposting,
  isPrivateRepostTarget,
  onRepost,
  isSelf,
  pinned,
  pinning,
  onTogglePin,
}: NoteCardActionsProps) {
  const { t } = useTranslation();

  return (
    <>
      <ReactionChips reactions={reactions} onToggle={onToggleReaction} disabled={reactionPending} />

      <div className={styles.actions}>
        <button className={styles.actionBtn} onClick={onReply} title={t("home:noteCard.replyButton")}>
          💬 {t("home:noteCard.replyButton")}
        </button>
        <button
          className={`${styles.actionBtn} ${reposted ? styles.actionBtnActive : ""}`}
          onClick={onRepost}
          disabled={reposting || unreposting || (isPrivateRepostTarget && !reposted)}
          title={
            isPrivateRepostTarget
              ? t("home:noteCard.repostDisabledTitle")
              : reposted
              ? t("home:noteCard.unrepostTitle")
              : t("home:noteCard.repostTitle")
          }
        >
          🔁{" "}
          {isPrivateRepostTarget
            ? t("home:noteCard.repostUnavailable")
            : reposted
            ? t("home:noteCard.reposted")
            : (reposting || unreposting)
            ? "..."
            : t("home:noteCard.repost")}
        </button>
        <ReactionPicker onPick={onToggleReaction} disabled={reactionPending} />
        {isSelf && (
          <button
            className={`${styles.actionBtn} ${pinned ? styles.actionBtnActive : ""}`}
            onClick={onTogglePin}
            disabled={pinning}
            title={pinned ? t("home:noteCard.unpinTitle") : t("home:noteCard.pinTitle")}
          >
            📌 {pinned ? t("home:noteCard.pinned") : pinning ? "..." : t("home:noteCard.pin")}
          </button>
        )}
      </div>
    </>
  );
}
