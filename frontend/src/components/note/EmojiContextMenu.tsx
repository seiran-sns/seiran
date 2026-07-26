import { cloneElement, isValidElement, MouseEvent as ReactMouseEvent, ReactElement, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { useAuth } from "../../contexts/AuthContext";
import { fetchCustomEmojiShortcodes } from "../../lib/customEmojis";
import EmojiImportDialog from "../admin/EmojiImportDialog";
import styles from "./EmojiContextMenu.module.css";

interface EmojiContextMenuProps {
  shortcode: string;
  imageUrl: string;
  children: ReactElement;
}

function hostnameOf(url: string): string | undefined {
  try {
    return new URL(url, window.location.origin).hostname;
  } catch {
    return undefined;
  }
}

/**
 * 本文内絵文字・絵文字リアクションを右クリックすると、管理者にのみリモート絵文字の
 * インポートメニューを出す（#73）。ローカル絵文字や非管理者には介入せず、渡された
 * 要素（`<img>`）をそのまま返す。
 *
 * ローカル/リモートの判定は画像URLのオリジンでは行わない。ローカルのカスタム絵文字も
 * S3/CDN 等の外部ストレージから配信されることがあり、その場合オリジン比較だと常に
 * 「リモート」と誤判定されてしまうため、このサーバーに登録済みの shortcode 一覧
 * （`fetchCustomEmojiShortcodes`）に含まれるかどうかで判定する。
 */
export default function EmojiContextMenu({ shortcode, imageUrl, children }: EmojiContextMenuProps) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [knownShortcodes, setKnownShortcodes] = useState<Set<string> | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    fetchCustomEmojiShortcodes().then(setKnownShortcodes);
  }, []);

  // 未ロード中は安全側（メニューを出さない）に倒す。
  const isRemote = knownShortcodes !== null && !knownShortcodes.has(shortcode);

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

  if (user?.role !== "admin" || !isRemote || !isValidElement(children)) {
    return children;
  }

  function handleContextMenu(e: ReactMouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    setMenuPos({ x: e.clientX, y: e.clientY });
  }

  return (
    <>
      {cloneElement(children, { onContextMenu: handleContextMenu } as Record<string, unknown>)}
      {menuPos &&
        createPortal(
          // NoteCard/ReactionChips 内にネストしたままだと祖先の transform 等が
          // 作るスタッキングコンテキストに閉じ込められ、z-index が効かず他のポップオーバーの
          // 裏に隠れてクリックも奪われてしまうため、body 直下へポータルして描画する。
          <div className={styles.popover} style={{ left: menuPos.x, top: menuPos.y }} ref={menuRef}>
            <button
              type="button"
              className={styles.item}
              onClick={(e) => {
                // このメニューはリアクションチップ（`<button>`）の内側にネストされることがあるため、
                // 親要素までクリックが伝播するとリアクションの追加/取消が誤発火し、その結果チップごと
                // アンマウントされてダイアログが開く前に消えてしまう。
                e.stopPropagation();
                setMenuPos(null);
                setDialogOpen(true);
              }}
            >
              {t("home:emojiContextMenu.importButton")}
            </button>
          </div>,
          document.body,
        )}
      {dialogOpen && (
        <EmojiImportDialog
          open
          onClose={() => setDialogOpen(false)}
          shortcode={shortcode}
          imageUrl={imageUrl}
          sourceLabel={hostnameOf(imageUrl)}
          onImported={() => setDialogOpen(false)}
        />
      )}
    </>
  );
}
