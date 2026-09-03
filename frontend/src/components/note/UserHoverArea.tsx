import { ReactNode } from "react";
import { useFollowHoverSwitch, FollowHoverTarget } from "../../hooks/useFollowHoverSwitch";
import UserHoverPopover from "./UserHoverPopover";
import styles from "./UserHoverPopover.module.css";

interface UserHoverAreaProps {
  target: FollowHoverTarget;
  isSelf: boolean;
  children: ReactNode;
}

/**
 * 任意の要素（アバターアイコン等）へのマウスオーバーで「フォロー状態スライドスイッチ」
 * （`UserHoverPopover`）を出す薄いラッパー。`NoteCard`の投稿者アイコンは独自に
 * `userContainer` 全体でホバー領域をまとめているためこれを使わないが、通知アイテムの
 * ように離れた場所にある複数のユーザーリンク（アバター・表示名）それぞれに個別で
 * 付ける場合はこちらを使う。
 *
 * `stopPropagation`しているのは、通知アイテムが`NoteHoverPreview`（投稿概要ポップアップ）
 * でも包まれているため、素通しすると外側のホバーも同時に発火してしまうのを防ぐため。
 */
export default function UserHoverArea({ target, isSelf, children }: UserHoverAreaProps) {
  const hover = useFollowHoverSwitch(target, isSelf);

  if (isSelf) return <>{children}</>;

  return (
    <span
      className={styles.wrap}
      onMouseEnter={(e) => {
        e.stopPropagation();
        hover.handleMouseEnter();
      }}
      onMouseLeave={(e) => {
        e.stopPropagation();
        hover.handleMouseLeave();
      }}
    >
      {hover.isHovered && (
        <UserHoverPopover
          followStatus={hover.followStatus}
          loadingStatus={hover.loadingStatus}
          followActionPending={hover.followActionPending}
          onToggle={hover.handleToggleFollow}
        />
      )}
      {children}
    </span>
  );
}
