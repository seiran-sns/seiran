import { cloneElement, isValidElement, MouseEvent as ReactMouseEvent, ReactElement, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAuth } from "../../contexts/AuthContext";
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
 * インポートメニューを出す（#73）。ローカル絵文字（画像URLが自インスタンス）や
 * 非管理者には介入せず、渡された要素（`<img>`）をそのまま返す。
 */
export default function EmojiContextMenu({ shortcode, imageUrl, children }: EmojiContextMenuProps) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  const isRemote = (() => {
    try {
      return new URL(imageUrl, window.location.origin).origin !== window.location.origin;
    } catch {
      return false;
    }
  })();

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
      {menuPos && (
        <div className={styles.popover} style={{ left: menuPos.x, top: menuPos.y }} ref={menuRef}>
          <button
            type="button"
            className={styles.item}
            onClick={() => {
              setMenuPos(null);
              setDialogOpen(true);
            }}
          >
            {t("home:emojiContextMenu.importButton")}
          </button>
        </div>
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
