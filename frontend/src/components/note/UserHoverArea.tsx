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
 * 通知アイテムが`NoteHoverPreview`（投稿概要ポップアップ）でも包まれている場合、
 * このホバー領域と外側のホバー領域が同時に発火すること自体は無害（両者は独立した
 * ポップアップとして共存できる）。逆に`onMouseEnter`/`onMouseLeave`で
 * `stopPropagation()`を呼ぶと、Reactの合成イベント実装（内部的に`mouseover`/
 * `mouseout`ネイティブイベントの伝播を経由してenter/leaveを計算する）が祖先要素への
 * 合成イベントのディスパッチ自体を打ち切ってしまい、外側の`NoteHoverPreview`の
 * `onMouseLeave`が呼ばれずポップアップが開いたまま残る不具合を引き起こす
 * （実機確認済みの回帰）。そのため`stopPropagation`は呼ばない。
 */
export default function UserHoverArea({ target, isSelf, children }: UserHoverAreaProps) {
  const hover = useFollowHoverSwitch(target, isSelf);

  if (isSelf) return <>{children}</>;

  return (
    <span
      className={styles.wrap}
      onMouseEnter={hover.handleMouseEnter}
      onMouseLeave={hover.handleMouseLeave}
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
