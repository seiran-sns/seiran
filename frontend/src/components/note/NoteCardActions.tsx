import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ReactionSummary } from "../../api/client";
import { formatCount } from "../../lib/format";
import ActionsMenu, { ActionsMenuItem } from "../common/ActionsMenu";
import Modal from "../common/Modal";
import ReactionChips from "./ReactionChips";
import ReactionPicker from "./ReactionPicker";
import ReportModal from "../report/ReportModal";
import TwemojiEmoji from "../common/TwemojiEmoji";
import styles from "./NoteCard.module.css";

interface NoteCardActionsProps {
  noteId: string;
  subjectActorId: string;
  subjectLabel: string;
  replyCount: number;
  quoteCount: number;
  repostCount: number;
  reactions: ReactionSummary[];
  reactionPending: boolean;
  onToggleReaction: (emoji: string) => void;
  onReply: (e?: React.MouseEvent) => void;
  onQuote: (e?: React.MouseEvent) => void;
  isPrivateQuoteTarget?: boolean;
  reposted: boolean;
  reposting: boolean;
  unreposting: boolean;
  isPrivateRepostTarget: boolean;
  onRepost: (e?: React.MouseEvent) => void;
  isSelf: boolean;
  pinned: boolean;
  pinning: boolean;
  onTogglePin: (e?: React.MouseEvent) => void;
  deleting: boolean;
  onDelete: () => void;
  /** リアクションチップに50pxインデントを付けるか（`ReactionChips`の`indent`をそのまま透過）。 */
  indent?: boolean;
}

/**
 * 投稿カード下部のリアクションチップ＋アクションボタン群（返信/リポスト/リアクション）＋
 * ケバブメニュー（返信/リポスト/リアクション/ピン留め/削除）。ピン留め・削除はメニューのみに
 * ある（自分の投稿のみ表示）。
 */
export default function NoteCardActions({
  noteId,
  subjectActorId,
  subjectLabel,
  replyCount,
  quoteCount,
  repostCount,
  reactions,
  reactionPending,
  onToggleReaction,
  onReply,
  onQuote,
  isPrivateQuoteTarget = false,
  reposted,
  reposting,
  unreposting,
  isPrivateRepostTarget,
  onRepost,
  isSelf,
  pinned,
  pinning,
  onTogglePin,
  deleting,
  onDelete,
  indent = true,
}: NoteCardActionsProps) {
  const { t } = useTranslation();
  const [reactionPickerOpen, setReactionPickerOpen] = useState(false);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [reportOpen, setReportOpen] = useState(false);
  const totalReactionCount = reactions.reduce((sum, r) => sum + r.count, 0);
  const reactedByMe = reactions.some((r) => r.reactedByMe);

  function confirmDelete() {
    setDeleteConfirmOpen(false);
    onDelete();
  }

  const menuItems: ActionsMenuItem[] = [
    {
      key: "reply",
      label: `💬 ${t("home:noteCard.replyButton")}`,
      onClick: () => onReply(),
    },
    {
      key: "quote",
      label: `❝ ${t("home:noteCard.quoteButton")}`,
      onClick: () => onQuote(),
      disabled: isPrivateQuoteTarget,
    },
    {
      key: "repost",
      label: `🔁 ${reposted ? t("home:noteCard.unrepostTitle") : t("home:noteCard.repostTitle")}`,
      onClick: () => onRepost(),
      disabled:
        reposting || unreposting || (isPrivateRepostTarget && !reposted),
    },
    {
      key: "reaction",
      label: `🙂 ${t("home:reactionPicker.addReactionButton")}`,
      onClick: () => setReactionPickerOpen(true),
      disabled: reactionPending,
    },
  ];
  if (isSelf) {
    menuItems.push({
      key: "pin",
      label: `📌 ${pinned ? t("home:noteCard.unpinTitle") : t("home:noteCard.pinTitle")}`,
      onClick: () => onTogglePin(),
      disabled: pinning,
    });
    menuItems.push({
      key: "delete",
      label: `🗑️ ${t("common:delete")}`,
      onClick: () => setDeleteConfirmOpen(true),
      disabled: deleting,
      danger: true,
    });
  }
  if (!isSelf) {
    menuItems.push({
      key: "report",
      label: `⚠️ ${t("home:noteCard.reportButton")}`,
      onClick: () => setReportOpen(true),
      danger: true,
    });
  }

  return (
    <>
      <ReactionChips
        noteId={noteId}
        reactions={reactions}
        onToggle={onToggleReaction}
        disabled={reactionPending}
        indent={indent}
      />

      <div className={styles.actions}>
        <button
          className={styles.actionBtn}
          onClick={onReply}
          title={t("home:noteCard.replyButton")}
        >
          <TwemojiEmoji emoji="💬" />{" "}
          {replyCount > 0 && (
            <span className={styles.actionCount}>{formatCount(replyCount)}</span>
          )}
        </button>
        <button
          className={styles.actionBtn}
          onClick={onQuote}
          disabled={isPrivateQuoteTarget}
          title={
            isPrivateQuoteTarget
              ? t("home:noteCard.quoteDisabledTitle")
              : t("home:noteCard.quoteButton")
          }
        >
          ❝{" "}
          {quoteCount > 0 && (
            <span className={styles.actionCount}>{formatCount(quoteCount)}</span>
          )}
        </button>
        <button
          className={`${styles.actionBtn} ${reposted ? styles.actionBtnActive : ""}`}
          onClick={onRepost}
          disabled={
            reposting || unreposting || (isPrivateRepostTarget && !reposted)
          }
          title={
            isPrivateRepostTarget
              ? t("home:noteCard.repostDisabledTitle")
              : reposted
                ? t("home:noteCard.unrepostTitle")
                : t("home:noteCard.repostTitle")
          }
        >
          <TwemojiEmoji emoji="🔁" />{" "}
          {repostCount > 0 && (
            <span className={styles.actionCount}>{formatCount(repostCount)}</span>
          )}
        </button>
        <ReactionPicker
          onPick={onToggleReaction}
          disabled={reactionPending}
          open={reactionPickerOpen}
          onOpenChange={setReactionPickerOpen}
          count={totalReactionCount}
          active={reactedByMe}
        />
        <ActionsMenu
          items={menuItems}
          triggerTitle={t("home:noteCard.menuTitle")}
          triggerClassName={styles.actionBtn}
        />
      </div>

      <Modal
        open={deleteConfirmOpen}
        onClose={() => setDeleteConfirmOpen(false)}
        title={t("home:noteCard.deleteConfirmModal.title")}
      >
        <p className={styles.modalText}>
          {t("home:noteCard.deleteConfirmModal.body")}
        </p>
        <div className={styles.modalActions}>
          <button
            className={styles.modalPrimaryDanger}
            onClick={confirmDelete}
            disabled={deleting}
          >
            {t("home:noteCard.deleteConfirmModal.confirmButton")}
          </button>
          <button
            className={styles.modalSecondary}
            onClick={() => setDeleteConfirmOpen(false)}
          >
            {t("common:cancel")}
          </button>
        </div>
      </Modal>
      <ReportModal
        open={reportOpen}
        onClose={() => setReportOpen(false)}
        subjectType="post"
        subjectActorId={subjectActorId}
        subjectPostId={noteId}
        subjectLabel={subjectLabel}
      />
    </>
  );
}
