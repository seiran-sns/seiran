import { useState } from "react";
import { api, getErrorMessage } from "../api/client";
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
}

/**
 * 投稿者アイコン等へのマウスオーバーで出す「フォロー状態スライドスイッチ」の
 * 状態・ロジック。`NoteCard`・通知アイテムのユーザーリンクの両方が共有する。
 */
export function useFollowHoverSwitch(target: FollowHoverTarget, isSelf: boolean) {
  const { showError } = useToast();
  const targetKey = profileQuery(target.username, target.domain);
  const followStatus = useFollowStatus(targetKey) ?? null;

  const [isHovered, setIsHovered] = useState(false);
  const [loadingStatus, setLoadingStatus] = useState(false);
  const [followActionPending, setFollowActionPending] = useState(false);

  function handleMouseEnter() {
    setIsHovered(true);
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

  function handleMouseLeave() {
    setIsHovered(false);
  }

  async function handleToggleFollow(e: React.MouseEvent) {
    e.stopPropagation();
    if (followActionPending || isSelf) return;

    setFollowActionPending(true);
    const current = followStatus ?? "not_following";

    try {
      if (followToggleAction(current) === "create") {
        const res = await api.follows.create(targetKey);
        setFollowStatusStore(targetKey, res.status === "accepted" ? "accepted" : "pending");
      } else {
        await api.follows.delete(targetKey);
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
    isHovered,
    loadingStatus,
    followActionPending,
    handleMouseEnter,
    handleMouseLeave,
    handleToggleFollow,
  };
}
