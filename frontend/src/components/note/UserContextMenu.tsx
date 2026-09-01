import { cloneElement, isValidElement, MouseEvent as ReactMouseEvent, ReactElement, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import Modal from "../common/Modal";
import { ActionsMenuPopoverList } from "../common/ActionsMenu";
import ReportModal from "../report/ReportModal";
import { useAuth } from "../../contexts/AuthContext";
import { useUserRelationshipMenu, UserRelationshipTarget } from "../../hooks/useUserRelationshipMenu";
import styles from "./UserContextMenu.module.css";

interface UserContextMenuProps {
  target: UserRelationshipTarget;
  children: ReactElement;
}

/**
 * NoteCardのユーザー名・アイコンを右クリックすると出す「対ユーザー操作メニュー」
 * （フォロー・ミュート・ブロック・リポストミュート・通報）。ProfilePageのケバブメニュー
 * （`ActionsMenu`）と同じ`useUserRelationshipMenu`フックを共有し、状態は
 * `stores/userRelationshipStore`経由で常に同期する。
 *
 * `EmojiContextMenu`と同じ理由（NoteCard内のtransform等が作るスタッキングコンテキストに
 * 閉じ込められ z-index が効かなくなるのを避けるため）で`document.body`直下へポータルする。
 */
export default function UserContextMenu({ target, children }: UserContextMenuProps) {
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const isSelf =
    !!currentUser &&
    currentUser.username === target.username &&
    (!target.domain || target.domain === window.location.hostname);

  const menu = useUserRelationshipMenu(target);

  useEffect(() => {
    if (!menuPos) return;
    function close(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuPos(null);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setMenuPos(null);
    }
    document.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [menuPos]);

  if (!currentUser || isSelf || !isValidElement(children)) {
    return children;
  }

  function handleContextMenu(e: ReactMouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    setMenuPos({ x: e.clientX, y: e.clientY });
  }

  return (
    <>
      {cloneElement(children, { onContextMenu: handleContextMenu } as Record<string, unknown>)}
      {menuPos &&
        createPortal(
          <div
            className={styles.popover}
            style={{ left: menuPos.x, top: menuPos.y }}
            ref={menuRef}
            onClick={(e) => e.stopPropagation()}
          >
            <ActionsMenuPopoverList
              items={menu.items}
              onPick={(item) => {
                setMenuPos(null);
                item.onClick();
              }}
            />
          </div>,
          document.body,
        )}
      {target.actorId && (
        <ReportModal
          open={menu.reportModalOpen}
          onClose={menu.closeReportModal}
          subjectType="actor"
          subjectActorId={target.actorId}
          subjectLabel={target.reportLabel}
        />
      )}
      <Modal
        open={menu.blockConfirmOpen}
        onClose={menu.closeBlockConfirm}
        title={t("profile:profilePage.blockConfirmModal.title")}
      >
        <p>{t("profile:profilePage.blockConfirmModal.body")}</p>
        <div className={styles.modalActions}>
          <button
            className={styles.modalPrimary}
            onClick={menu.confirmBlock}
            disabled={menu.blockActionLoading}
          >
            {t("profile:profilePage.blockConfirmModal.confirmButton")}
          </button>
          <button className={styles.modalSecondary} onClick={menu.closeBlockConfirm}>
            {t("profile:profilePage.blockConfirmModal.cancelButton")}
          </button>
        </div>
      </Modal>
    </>
  );
}
