import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ReactionSummary } from "../../api/client";
import ActionsMenu, { ActionsMenuItem } from "../common/ActionsMenu";
import Modal from "../common/Modal";
import ReactionChips from "./ReactionChips";
import ReactionPicker from "./ReactionPicker";
import styles from "./NoteCard.module.css";

interface NoteCardActionsProps {
  noteId: string;
  reactions: ReactionSummary[];
  reactionPending: boolean;
  onToggleReaction: (emoji: string) => void;
  onReply: (e?: React.MouseEvent) => void;
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
}

/**
 * 投稿カード下部のリアクションチップ＋アクションボタン群（返信/リポスト/リアクション）＋
 * ケバブメニュー（返信/リポスト/リアクション/ピン留め/削除）。ピン留め・削除はメニューのみに
 * ある（自分の投稿のみ表示）。
 */
export default function NoteCardActions({
  noteId,
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
  deleting,
  onDelete,
}: NoteCardActionsProps) {
  const { t } = useTranslation();
  const [reactionPickerOpen, setReactionPickerOpen] = useState(false);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);

  function confirmDelete() {
    setDeleteConfirmOpen(false);
    onDelete();
  }

  const menuItems: ActionsMenuItem[] = [
    { key: "reply", label: `💬 ${t("home:noteCard.replyButton")}`, onClick: () => onReply() },
    {
      key: "repost",
      label: `🔁 ${reposted ? t("home:noteCard.unrepostTitle") : t("home:noteCard.repostTitle")}`,
      onClick: () => onRepost(),
      disabled: reposting || unreposting || (isPrivateRepostTarget && !reposted),
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

  return (
    <>
      <ReactionChips noteId={noteId} reactions={reactions} onToggle={onToggleReaction} disabled={reactionPending} />

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
        <ReactionPicker
          onPick={onToggleReaction}
          disabled={reactionPending}
          open={reactionPickerOpen}
          onOpenChange={setReactionPickerOpen}
        />
        <ActionsMenu items={menuItems} triggerTitle={t("home:noteCard.menuTitle")} />
      </div>

      <Modal
        open={deleteConfirmOpen}
        onClose={() => setDeleteConfirmOpen(false)}
        title={t("home:noteCard.deleteConfirmModal.title")}
      >
        <p className={styles.modalText}>{t("home:noteCard.deleteConfirmModal.body")}</p>
        <div className={styles.modalActions}>
          <button className={styles.modalPrimaryDanger} onClick={confirmDelete} disabled={deleting}>
            {t("home:noteCard.deleteConfirmModal.confirmButton")}
          </button>
          <button className={styles.modalSecondary} onClick={() => setDeleteConfirmOpen(false)}>
            {t("common:cancel")}
          </button>
        </div>
      </Modal>
    </>
  );
}
