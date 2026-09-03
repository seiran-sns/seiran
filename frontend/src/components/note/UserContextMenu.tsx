import { cloneElement, isValidElement, ReactElement } from "react";
import { UserRelationshipTarget } from "../../hooks/useUserRelationshipMenu";
import { useUserContextMenuPopover } from "../../hooks/useUserContextMenuPopover";
import UserRelationshipPopovers from "./UserRelationshipPopovers";

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
  const state = useUserContextMenuPopover(target);
  const { currentUser, isSelf, handleContextMenu } = state;

  if (!currentUser || isSelf || !isValidElement(children)) {
    return children;
  }

  return (
    <>
      {cloneElement(children, { onContextMenu: handleContextMenu } as Record<string, unknown>)}
      <UserRelationshipPopovers target={target} state={state} />
    </>
  );
}
