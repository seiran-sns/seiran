import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import Modal from "../common/Modal";
import { ActionsMenuPopoverList } from "../common/ActionsMenu";
import ReportModal from "../report/ReportModal";
import { UserRelationshipTarget } from "../../hooks/useUserRelationshipMenu";
import { useUserContextMenuPopover } from "../../hooks/useUserContextMenuPopover";
import styles from "./UserContextMenu.module.css";

interface UserRelationshipPopoversProps {
  target: UserRelationshipTarget;
  state: ReturnType<typeof useUserContextMenuPopover>;
}

/**
 * 対ユーザー右クリックメニュー（`ActionsMenuPopoverList`のポータル）＋
 * 通報モーダル＋ブロック確認モーダル。`UserContextMenu`・`UserLinkTag`が共有する。
 */
export default function UserRelationshipPopovers({ target, state }: UserRelationshipPopoversProps) {
  const { t } = useTranslation();
  const { menu, menuPos, menuRef, closeMenu } = state;

  return (
    <>
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
                closeMenu();
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
