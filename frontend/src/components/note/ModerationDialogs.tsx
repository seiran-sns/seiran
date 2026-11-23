import { useTranslation } from "react-i18next";
import ConfirmModal from "../common/ConfirmModal";
import { useUserRelationshipMenu } from "../../hooks/useUserRelationshipMenu";
import styles from "./ModerationDialogs.module.css";

interface ModerationDialogsProps {
  menu: ReturnType<typeof useUserRelationshipMenu>;
  /** 対象の表示（`@user@host`）。 */
  subjectLabel: string;
}

/** ユーザー操作メニューの凍結・凍結解除・ロール変更の確認ダイアログ（調停者向け）。 */
export default function ModerationDialogs({ menu, subjectLabel }: ModerationDialogsProps) {
  const { t } = useTranslation();
  const dialog = menu.moderationDialog;
  const moderation = menu.moderation;
  if (!dialog || !moderation) return null;

  if (dialog.kind === "role") {
    return (
      <ConfirmModal
        open
        onClose={menu.closeModerationDialog}
        title={t("profile:moderation.changeRoleTitle")}
        confirmLabel={t("profile:moderation.changeRoleConfirm")}
        onConfirm={menu.confirmModeration}
        busy={menu.moderationBusy || dialog.role === moderation.role}
      >
        <p>{t("profile:moderation.changeRoleBody", { subject: subjectLabel, role: moderation.role })}</p>
        <select
          className={styles.select}
          value={dialog.role}
          onChange={(e) => menu.selectModerationRole(e.target.value)}
        >
          {moderation.assignableRoles.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
      </ConfirmModal>
    );
  }

  const suspend = dialog.kind === "suspend";
  return (
    <ConfirmModal
      open
      onClose={menu.closeModerationDialog}
      title={t(suspend ? "profile:moderation.suspendTitle" : "profile:moderation.unsuspendTitle")}
      confirmLabel={t(suspend ? "profile:moderation.suspendConfirm" : "profile:moderation.unsuspendConfirm")}
      onConfirm={menu.confirmModeration}
      busy={menu.moderationBusy}
    >
      <p>
        {t(suspend ? "profile:moderation.suspendBody" : "profile:moderation.unsuspendBody", {
          subject: subjectLabel,
        })}
      </p>
    </ConfirmModal>
  );
}
