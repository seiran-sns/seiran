import { useState, type CSSProperties } from "react";
import { api, getErrorMessage } from "../api/client";
import { followTargetOf } from "../api/follows";
import { useToast } from "../contexts/ToastContext";
import { profileQuery } from "../lib/format";
import {
  setFollowStatus as setFollowStatusStore,
  useFollowStatus,
} from "../stores/followStatusStore";
import { FollowStatus, setRelationship } from "../stores/userRelationshipStore";

export function followToggleAction(status: FollowStatus | null): "create" | "delete" {
  return status === null || status === "not_following" ? "create" : "delete";
}

export interface FollowHoverTarget {
  username: string;
  domain?: string;
  /** あればフォロー操作の対象指定に使う（`username`/`domain`由来の文字列より優先）。 */
  actorId?: string;
}

/**
 * 投稿者アイコン等へのマウスオーバーで出す「フォロー状態スライドスイッチ」の
 * 状態・ロジック。`NoteCard`・通知アイテムのユーザーリンクの両方が共有する。
 */
export function useFollowHoverSwitch(target: FollowHoverTarget, isSelf: boolean) {
  const { showError } = useToast();
  const targetKey = profileQuery(target.username, target.domain);
  const followStatus = useFollowStatus(targetKey) ?? null;

  // 一度でもマウスが乗ったらポップアップをDOMに置く。表示・非表示はCSSの`:hover`が
  // 決める（UserHoverPopover.module.cssの`.host:hover > .popover`）。enter/leaveの対で
  // 開閉ステートを持つと、leaveを取りこぼしたときに閉じる契機が来ず残り続けるため。
  const [activated, setActivated] = useState(false);
  const [popoverStyle, setPopoverStyle] = useState<CSSProperties | undefined>(undefined);
  const [loadingStatus, setLoadingStatus] = useState(false);
  const [followActionPending, setFollowActionPending] = useState(false);

  function handleMouseEnter(e: React.MouseEvent<HTMLElement>) {
    // ホバー領域の真上へ画面基準（position: fixed）で置く。通知アイテムの名前は
    // 省略記号を出すためoverflow: hiddenの1行（NotificationsPanelの.singleLine）の中に
    // あり、その外へはみ出す絶対配置だと切り取られて見えなくなる。
    const rect = e.currentTarget.getBoundingClientRect();
    setPopoverStyle({
      position: "fixed",
      left: rect.left,
      bottom: document.documentElement.clientHeight - rect.top + 4,
    });
    setActivated(true);
    if (!isSelf && followStatus === null && !loadingStatus) {
      setLoadingStatus(true);
      api.users
        .profile(targetKey)
        .then((p) =>
          setRelationship(targetKey, {
            followStatus: p.follow_status,
            isMuted: p.is_muted,
            isBlocking: p.is_blocking,
            isBlockedBy: p.is_blocked_by,
            isRepostMuted: p.is_repost_muted,
          }),
        )
        .catch(() => setFollowStatusStore(targetKey, "not_following"))
        .finally(() => setLoadingStatus(false));
    }
  }

  async function handleToggleFollow(e: React.MouseEvent) {
    e.stopPropagation();
    if (followActionPending || isSelf) return;

    setFollowActionPending(true);
    const current = followStatus ?? "not_following";

    try {
      if (followToggleAction(current) === "create") {
        const res = await api.follows.create(followTargetOf(target.actorId, targetKey));
        setFollowStatusStore(targetKey, res.status === "accepted" ? "accepted" : "pending");
      } else {
        await api.follows.delete(followTargetOf(target.actorId, targetKey));
        setFollowStatusStore(targetKey, "not_following");
      }
    } catch (err) {
      showError(getErrorMessage(err));
    } finally {
      setFollowActionPending(false);
    }
  }

  return {
    followStatus,
    activated,
    popoverStyle,
    loadingStatus,
    followActionPending,
    handleMouseEnter,
    handleToggleFollow,
  };
}
