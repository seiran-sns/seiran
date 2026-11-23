import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, ListMembership } from "../api/client";
import { followTargetOf } from "../api/follows";
import { useAuth } from "../contexts/AuthContext";
import { useToast } from "../contexts/ToastContext";
import { profileQuery } from "../lib/format";
import { getAdminTopics, isPrivilegedRole } from "../lib/roles";
import { ActionsMenuItem } from "../components/common/ActionsMenu";
import {
  FollowStatus,
  RelationshipSnapshot,
  setRelationship,
  useRelationship,
} from "../stores/userRelationshipStore";

/** メニューに出す「リストに追加/から外す」項目の最大数。作ったリストが多い
 * ユーザーでもメニューが画面をはみ出さないよう、先頭（作成順）から5件までに絞る。
 * 6件目以降はプロフィール画面かリスト設定画面から操作する。 */
const MAX_LIST_MENU_ITEMS = 5;

const DEFAULT_RELATIONSHIP: RelationshipSnapshot = {
  followStatus: "not_following",
  isMuted: false,
  isBlocking: false,
  isBlockedBy: false,
  isRepostMuted: false,
};

export interface UserRelationshipTarget {
  /** フォロー操作の対象指定（あれば`target`より優先）とreport用。無ければreport項目をdisabledにする。 */
  actorId?: string;
  username: string;
  domain?: string;
  /** API呼び出し用target文字列。省略時は `profileQuery(username, domain)` を使う
   * （ProfilePageはap_uri/at_did優先の既存ロジックのため明示指定、NoteCard発は省略）。 */
  target?: string;
  /** trueならフォロー時にブリッジユーザー確認モーダルを挟む（ProfilePageのみ利用）。 */
  isBridge?: boolean;
  reportLabel: string;
}

/**
 * ProfilePageのケバブメニューとNoteCardの対ユーザー右クリックメニュー
 * （`UserContextMenu`）の両方が共有する「対ユーザー操作」の状態・ロジック・
 * `ActionsMenuItem[]`を1箇所に集約するフック。JSX（`ReportModal`・ブロック確認
 * モーダル）は持たず、開閉フラグと確定アクションのみ返す。
 */
export function useUserRelationshipMenu(
  target: UserRelationshipTarget,
  options?: { onChanged?: (patch: Partial<RelationshipSnapshot>) => void },
) {
  const { t } = useTranslation();
  const { showError } = useToast();

  const key = profileQuery(target.username, target.domain);
  const relationship = useRelationship(key) ?? null;
  const apiTarget = target.target ?? key;

  const [followActionPending, setFollowActionPending] = useState(false);
  const [muteActionLoading, setMuteActionLoading] = useState(false);
  const [repostMuteActionLoading, setRepostMuteActionLoading] = useState(false);
  const [blockActionLoading, setBlockActionLoading] = useState(false);
  const [blockConfirmOpen, setBlockConfirmOpen] = useState(false);
  const [reportModalOpen, setReportModalOpen] = useState(false);

  const [listMemberships, setListMemberships] = useState<ListMembership[] | null>(null);
  // 調停者（admin・moderator）向けの凍結・ロール変更。メニューを開いたときに対象の状態を取る。
  const { user: currentUser } = useAuth();
  const canModerate = getAdminTopics(currentUser?.role).includes("reports");
  const [moderation, setModeration] = useState<ModerationState | null>(null);
  const [moderationDialog, setModerationDialog] = useState<ModerationDialog | null>(null);
  const [moderationBusy, setModerationBusy] = useState(false);
  const [listActionPendingId, setListActionPendingId] = useState<string | null>(null);

  function patch(p: Partial<RelationshipSnapshot>) {
    const prev = relationship ?? DEFAULT_RELATIONSHIP;
    setRelationship(key, { ...prev, ...p });
    options?.onChanged?.(p);
  }

  const followStatus: FollowStatus = relationship?.followStatus ?? "not_following";

  async function doFollow() {
    if (followActionPending) return;
    setFollowActionPending(true);
    try {
      const res = await api.follows.create(followTargetOf(target.actorId, apiTarget));
      patch({ followStatus: res.status === "accepted" ? "accepted" : "pending" });
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setFollowActionPending(false);
    }
  }

  async function doUnfollow() {
    if (followActionPending) return;
    setFollowActionPending(true);
    try {
      await api.follows.delete(followTargetOf(target.actorId, apiTarget));
      patch({ followStatus: "not_following" });
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setFollowActionPending(false);
    }
  }

  async function doMute() {
    setMuteActionLoading(true);
    try {
      await api.mutes.create(apiTarget);
      patch({ isMuted: true });
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setMuteActionLoading(false);
    }
  }

  async function doUnmute() {
    setMuteActionLoading(true);
    try {
      await api.mutes.delete(apiTarget);
      patch({ isMuted: false });
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setMuteActionLoading(false);
    }
  }

  async function doRepostMute() {
    setRepostMuteActionLoading(true);
    try {
      await api.repostMutes.create(apiTarget);
      patch({ isRepostMuted: true });
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setRepostMuteActionLoading(false);
    }
  }

  async function doUnrepostMute() {
    setRepostMuteActionLoading(true);
    try {
      await api.repostMutes.delete(apiTarget);
      patch({ isRepostMuted: false });
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setRepostMuteActionLoading(false);
    }
  }

  // ブロックは相互フォロー強制解除を伴う破壊的操作のため、確認モーダルを経由してから実行する。
  // バックエンドが双方向のフォロー関係を強制解除するので、followStatusもここで
  // not_followingへ更新する（onChangedを渡さない呼び出し元でも正しく反映されるように）。
  async function confirmBlock() {
    setBlockActionLoading(true);
    try {
      await api.blocks.create(apiTarget);
      patch({ isBlocking: true, followStatus: "not_following" });
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setBlockActionLoading(false);
      setBlockConfirmOpen(false);
    }
  }

  async function doUnblock() {
    setBlockActionLoading(true);
    try {
      await api.blocks.delete(apiTarget);
      patch({ isBlocking: false });
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setBlockActionLoading(false);
    }
  }

  // 右クリック/ケバブメニューを開いた時点で呼ぶ想定（`actorId`が無いリモート未登録
  // アクター等はリスト項目自体を出さない）。メニューを開くたびに毎回取得し直すため、
  // NoteCard等の一覧画面でマウント時に全件分のリクエストが飛ぶことはない。
  async function loadListMemberships() {
    if (!target.actorId) return;
    try {
      setListMemberships(await api.lists.membership(target.actorId));
    } catch {
      // 失敗してもメニュー自体は成立させる（リスト項目だけ出さない）。
    }
  }

  async function loadModeration() {
    if (!canModerate || !target.actorId) return;
    try {
      const res = await api.admin.actorModeration(target.actorId);
      setModeration({ suspended: res.suspended, role: res.role, assignableRoles: res.assignable_roles });
    } catch {
      // 失敗しても他の項目は出す（凍結・ロール変更の項目だけ出さない）。
    }
  }

  /** メニューを開いたときに、開いてから決まる項目（リスト・調停）の材料を取る。 */
  function loadMenuData() {
    void loadListMemberships();
    void loadModeration();
  }

  async function confirmModeration() {
    if (!moderationDialog || !target.actorId || !moderation) return;
    setModerationBusy(true);
    try {
      if (moderationDialog.kind === "suspend") {
        await api.admin.suspendActor(target.actorId);
        setModeration({ ...moderation, suspended: true });
      } else if (moderationDialog.kind === "unsuspend") {
        await api.admin.unsuspendActor(target.actorId);
        setModeration({ ...moderation, suspended: false });
      } else {
        await api.admin.changeActorRole(target.actorId, moderationDialog.role);
        setModeration({ ...moderation, role: moderationDialog.role });
      }
      setModerationDialog(null);
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setModerationBusy(false);
    }
  }

  async function toggleListMembership(list: ListMembership) {
    if (!target.actorId || listActionPendingId) return;
    setListActionPendingId(list.id);
    try {
      if (list.contains) {
        await api.lists.removeMember(list.id, target.actorId);
      } else {
        await api.lists.addMember(list.id, apiTarget);
      }
      setListMemberships(
        (prev) => prev?.map((l) => (l.id === list.id ? { ...l, contains: !l.contains } : l)) ?? null,
      );
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setListActionPendingId(null);
    }
  }

  const items: ActionsMenuItem[] = [];

  if (followStatus === "accepted" || followStatus === "pending") {
    items.push({
      key: followStatus === "accepted" ? "unfollow" : "cancel-pending-follow",
      label: followActionPending
        ? t("profile:profilePage.unfollowingButton")
        : t("profile:profilePage.unfollowButton"),
      onClick: doUnfollow,
      disabled: followActionPending,
    });
  } else {
    items.push({
      key: "follow",
      label: followActionPending
        ? t("profile:profilePage.followingSubmitButton")
        : t("profile:profilePage.followButton"),
      onClick: doFollow,
      disabled: followActionPending,
    });
  }

  items.push(
    relationship?.isMuted
      ? {
          key: "unmute",
          label: t("profile:profilePage.unmuteButton"),
          onClick: doUnmute,
          disabled: muteActionLoading,
        }
      : {
          key: "mute",
          label: t("profile:profilePage.muteButton"),
          onClick: doMute,
          disabled: muteActionLoading,
        },
  );

  items.push(
    relationship?.isRepostMuted
      ? {
          key: "unrepost-mute",
          label: t("profile:profilePage.unrepostMuteButton"),
          onClick: doUnrepostMute,
          disabled: repostMuteActionLoading,
        }
      : {
          key: "repost-mute",
          label: t("profile:profilePage.repostMuteButton"),
          onClick: doRepostMute,
          disabled: repostMuteActionLoading,
        },
  );

  for (const list of (listMemberships ?? []).slice(0, MAX_LIST_MENU_ITEMS)) {
    items.push({
      key: `list-${list.id}`,
      label: list.contains
        ? t("profile:profilePage.removeFromListButton", { name: list.name })
        : t("profile:profilePage.addToListButton", { name: list.name }),
      title: list.name,
      onClick: () => toggleListMembership(list),
      disabled: listActionPendingId === list.id,
    });
  }

  items.push(
    relationship?.isBlocking
      ? {
          key: "unblock",
          label: t("profile:profilePage.unblockButton"),
          onClick: doUnblock,
          danger: true,
          disabled: blockActionLoading,
        }
      : {
          key: "block",
          label: t("profile:profilePage.blockButton"),
          onClick: () => setBlockConfirmOpen(true),
          danger: true,
          disabled: blockActionLoading,
        },
  );

  if (moderation) {
    if (moderation.suspended) {
      items.push({
        key: "unsuspend",
        label: `🛡️ ${t("profile:moderation.unsuspendItem")}`,
        onClick: () => setModerationDialog({ kind: "unsuspend" }),
        danger: true,
      });
    } else if (!isPrivilegedRole(moderation.role)) {
      items.push({
        key: "suspend",
        label: `🛡️ ${t("profile:moderation.suspendItem")}`,
        onClick: () => setModerationDialog({ kind: "suspend" }),
        danger: true,
      });
    }
    if (moderation.role && moderation.assignableRoles.length > 0) {
      const current = moderation.role;
      items.push({
        key: "change-role",
        label: `🛡️ ${t("profile:moderation.changeRoleItem", { role: current })}`,
        onClick: () => setModerationDialog({ kind: "role", role: current }),
        danger: true,
      });
    }
  }

  items.push({
    key: "report",
    label: `⚠️ ${t("home:noteCard.reportButton")}`,
    onClick: () => setReportModalOpen(true),
    danger: true,
    disabled: !target.actorId,
  });

  return {
    relationship,
    items,
    followStatus,
    doFollow,
    doUnfollow,
    followActionPending,
    blockConfirmOpen,
    closeBlockConfirm: () => setBlockConfirmOpen(false),
    confirmBlock,
    blockActionLoading,
    reportModalOpen,
    closeReportModal: () => setReportModalOpen(false),

    loadMenuData,

    moderation,
    moderationDialog,
    /** ロール変更ダイアログで選んだロールを反映する。 */
    selectModerationRole: (role: string) =>
      setModerationDialog((d) => (d?.kind === "role" ? { kind: "role", role } : d)),
    closeModerationDialog: () => setModerationDialog(null),
    confirmModeration,
    moderationBusy,
  };
}

/** 調停者向け項目の材料（`GET /api/admin/actors/:id/moderation`）。 */
export interface ModerationState {
  suspended: boolean;
  /** ローカルユーザーならロール（リモートは null）。 */
  role: string | null;
  /** 操作者が付けられるロール。空ならロール変更の項目を出さない。 */
  assignableRoles: string[];
}

/** 開いている確認ダイアログ。`role` は選んでいる変更後のロール。 */
export type ModerationDialog = { kind: "suspend" } | { kind: "unsuspend" } | { kind: "role"; role: string };
