import { useEffect, useRef, useState } from "react";
import styles from "./ActionsMenu.module.css";

export interface ActionsMenuItem {
  key: string;
  label: string;
  onClick: () => void;
  /** 破壊的操作（ブロック等）は赤字表示にする。 */
  danger?: boolean;
  disabled?: boolean;
}

interface ActionsMenuProps {
  items: ActionsMenuItem[];
  /** トリガーボタンの表示文字。デフォルトはケバブメニュー（⋯）。 */
  triggerLabel?: string;
  triggerTitle?: string;
}

/** トリガーボタン＋ポップオーバー形式の汎用アクションメニュー（`ReactionPicker` のパターンを踏襲）。 */
export default function ActionsMenu({ items, triggerLabel = "⋯", triggerTitle }: ActionsMenuProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    function handleOutsideClick(e: MouseEvent) {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", handleOutsideClick);
    return () => document.removeEventListener("mousedown", handleOutsideClick);
  }, [open]);

  function pick(item: ActionsMenuItem) {
    if (item.disabled) return;
    setOpen(false);
    item.onClick();
  }

  return (
    <div className={styles.wrap} ref={wrapRef}>
      <button
        type="button"
        className={styles.trigger}
        title={triggerTitle}
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => !v);
        }}
      >
        {triggerLabel}
      </button>
      {open && (
        <div className={styles.popover} onClick={(e) => e.stopPropagation()}>
          {items.map((item) => (
            <button
              key={item.key}
              type="button"
              className={`${styles.item} ${item.danger ? styles.itemDanger : ""}`}
              disabled={item.disabled}
              onClick={() => pick(item)}
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
