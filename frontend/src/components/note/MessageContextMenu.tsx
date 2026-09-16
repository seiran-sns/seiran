import {
  cloneElement,
  isValidElement,
  MouseEvent as ReactMouseEvent,
  ReactElement,
  useEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import styles from "./MessageContextMenu.module.css";

interface MessageContextMenuProps {
  children: ReactElement;
  onReact: () => void;
  onDelete: () => void;
  /** 自分自身が送ったメッセージのみ削除（bsky宛は「隠す」）できる。 */
  canDelete: boolean;
  /** bsky宛/bsky発のスレッドか。true の間はリアクション・削除/隠すともに無効化する
   * （`chat.bsky.convo.addReaction`/`deleteMessageForSelf`未対応、後日対応予定）。 */
  isBsky: boolean;
}

/** メッセージ画面（DM）のメッセージ右クリックメニュー。「リアクション」「削除」（bsky宛は
 * 「隠す」）を出す。`EmojiContextMenu`と同じ、`createPortal`でbody直下に描画するパターン。 */
export default function MessageContextMenu({
  children,
  onReact,
  onDelete,
  canDelete,
  isBsky,
}: MessageContextMenuProps) {
  const { t } = useTranslation();
  const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);

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

  if (!isValidElement(children)) return children;

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
          <div className={styles.popover} style={{ left: menuPos.x, top: menuPos.y }} ref={menuRef}>
            <button
              type="button"
              className={styles.item}
              disabled={isBsky}
              title={isBsky ? t("dm:messagesPage.contextMenu.bskyUnsupported") : undefined}
              onClick={(e) => {
                e.stopPropagation();
                setMenuPos(null);
                onReact();
              }}
            >
              {t("dm:messagesPage.contextMenu.react")}
            </button>
            {canDelete && (
              <button
                type="button"
                className={`${styles.item} ${styles.itemDanger}`}
                disabled={isBsky}
                title={isBsky ? t("dm:messagesPage.contextMenu.bskyUnsupported") : undefined}
                onClick={(e) => {
                  e.stopPropagation();
                  setMenuPos(null);
                  onDelete();
                }}
              >
                {isBsky
                  ? t("dm:messagesPage.contextMenu.hide")
                  : t("dm:messagesPage.contextMenu.delete")}
              </button>
            )}
          </div>,
          document.body,
        )}
    </>
  );
}
