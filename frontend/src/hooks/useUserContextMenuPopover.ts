import { MouseEvent as ReactMouseEvent, useEffect, useRef, useState } from "react";
import { useAuth } from "../contexts/AuthContext";
import { useUserRelationshipMenu, UserRelationshipTarget } from "./useUserRelationshipMenu";

/**
 * 対ユーザー右クリックメニュー（フォロー・ミュート・ブロック・リポストミュート・通報）の
 * ポップオーバー位置・開閉ロジック。`UserContextMenu`（NoteCard）・通知アイテムの
 * ユーザーリンクの両方が共有する。
 */
export function useUserContextMenuPopover(target: UserRelationshipTarget) {
  const { user: currentUser } = useAuth();
  const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const isSelf =
    !!currentUser &&
    currentUser.username === target.username &&
    (!target.domain || target.domain === window.location.hostname);

  const menu = useUserRelationshipMenu(target);

  useEffect(() => {
    if (!menuPos) return;
    function close(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) setMenuPos(null);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setMenuPos(null);
    }
    document.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [menuPos]);

  function handleContextMenu(e: ReactMouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    setMenuPos({ x: e.clientX, y: e.clientY });
  }

  function closeMenu() {
    setMenuPos(null);
  }

  return { currentUser, isSelf, menu, menuPos, menuRef, handleContextMenu, closeMenu };
}
