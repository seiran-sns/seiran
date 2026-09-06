import { UserRelationshipTarget } from "../hooks/useUserRelationshipMenu";

interface RelationshipUserLike {
  id: string;
  username: string;
  host?: string | null;
}

/**
 * ユーザー情報を対ユーザー操作メニュー用の`UserRelationshipTarget`に変換する。
 * `NotificationsPanel`・`ReactionEventCard`など、投稿者以外の相手ユーザーが並ぶ
 * 一覧アイテムで共有する（`UserContextMenu`/`UserLinkTag`へ渡す）。
 */
export function toRelationshipTarget(u: RelationshipUserLike): UserRelationshipTarget {
  return {
    username: u.username,
    domain: u.host ?? undefined,
    actorId: u.id,
    reportLabel: `@${u.username}${u.host ? `@${u.host}` : ""}`,
  };
}

/** 一覧アイテムに出てくるユーザーが閲覧者自身かどうか（`UserHoverArea`のisSelf判定用）。 */
export function isSelfUser(
  currentUser: { username: string } | null,
  u?: { username: string; host?: string | null }
): boolean {
  return (
    !!currentUser && !!u && currentUser.username === u.username && (!u.host || u.host === window.location.hostname)
  );
}
