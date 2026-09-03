import { ReactNode } from "react";
import { Link } from "react-router-dom";
import { useFollowHoverSwitch } from "../../hooks/useFollowHoverSwitch";
import { UserRelationshipTarget } from "../../hooks/useUserRelationshipMenu";
import { useUserContextMenuPopover } from "../../hooks/useUserContextMenuPopover";
import UserHoverPopover from "./UserHoverPopover";
import UserRelationshipPopovers from "./UserRelationshipPopovers";
import hoverStyles from "./UserHoverPopover.module.css";

interface UserLinkTagProps {
  target: UserRelationshipTarget;
  to: string;
  className?: string;
  children?: ReactNode;
}

/**
 * react-i18next の `Trans` コンポーネントへ `<userLink>` タグとして渡す用の
 * プロフィールリンク。`Trans`はタグ要素の`children`を翻訳文の該当部分の内容
 * （テキスト・他コンポーネント）で上書きするため、`UserContextMenu`
 * （`children`に固定の要素を渡す方式）はここでは使えず、代わりに`children`を
 * そのまま`Link`の中へ描画する形で組む。ホバー時のフォロースイッチ・右クリックの
 * 対ユーザー操作メニューは`NoteCard`/`UserContextMenu`と同じフックを共有する。
 */
export default function UserLinkTag({ target, to, className, children }: UserLinkTagProps) {
  const state = useUserContextMenuPopover(target);
  const { currentUser, isSelf, handleContextMenu } = state;
  const hover = useFollowHoverSwitch(target, isSelf);

  if (!currentUser || isSelf) {
    return (
      <Link to={to} className={className} onClick={(e) => e.stopPropagation()}>
        {children}
      </Link>
    );
  }

  return (
    <span
      className={hoverStyles.wrap}
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
      <Link
        to={to}
        className={className}
        onClick={(e) => e.stopPropagation()}
        onContextMenu={handleContextMenu}
      >
        {children}
      </Link>
      <UserRelationshipPopovers target={target} state={state} />
    </span>
  );
}
