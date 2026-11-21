import { useEffect, useLayoutEffect, useRef, useState } from "react";
import TwemojiText from "./TwemojiText";
import styles from "./ActionsMenu.module.css";

export interface ActionsMenuItem {
  key: string;
  label: string;
  onClick: () => void;
  /** 破壊的操作（ブロック等）は赤字表示にする。 */
  danger?: boolean;
  disabled?: boolean;
  /** ボタン(title属性)でのホバー表示。labelを`overflow:hidden`で省略表示する項目
   * （リスト名など）で、省略前の全文を見せるために使う。 */
  title?: string;
}

interface ActionsMenuProps {
  items: ActionsMenuItem[];
  /** トリガーボタンの表示文字。デフォルトはケバブメニュー（⋯）。 */
  triggerLabel?: string;
  triggerTitle?: string;
  /** 指定時、トリガーボタンのデフォルトの見た目（`styles.trigger`）の代わりに使う
   * （呼び出し元の他のボタンと体裁を揃えたい場合）。 */
  triggerClassName?: string;
  /** メニューを開いた瞬間（トリガーボタン押下でpopoverが開く時）に呼ぶ。
   * リスト所属状態のような、開くたびに取り直したい遅延データの取得に使う。 */
  onOpen?: () => void;
}

/** items一覧のボタン描画部分（ケバブメニューの`ActionsMenu`・NoteCardの右クリック
 * メニュー`UserContextMenu`の両方から使う共通プレゼンテーション部品）。 */
export function ActionsMenuPopoverList({
  items,
  onPick,
}: {
  items: ActionsMenuItem[];
  onPick: (item: ActionsMenuItem) => void;
}) {
  return (
    <>
      {items.map((item) => (
        <button
          key={item.key}
          type="button"
          className={`${styles.item} ${item.danger ? styles.itemDanger : ""}`}
          disabled={item.disabled}
          title={item.title}
          onClick={() => onPick(item)}
        >
          <TwemojiText text={item.label} />
        </button>
      ))}
    </>
  );
}

/** ポップオーバーを画面の端から離す最小の余白（px）。 */
const VIEWPORT_MARGIN = 8;

/** トリガーボタン＋ポップオーバー形式の汎用アクションメニュー（`ReactionPicker` のパターンを踏襲）。
 *
 * ポップオーバーはトリガーの右端にそろえて左へ開く。トリガーが画面の左寄りにあって左端から
 * はみ出すときは、はみ出す分だけ右へずらす。 */
export default function ActionsMenu({
  items,
  triggerLabel = "⋯",
  triggerTitle,
  triggerClassName,
  onOpen,
}: ActionsMenuProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  // 右へずらす量（px）。開くたびに右端そろえの位置から測り直す。
  const [shift, setShift] = useState(0);

  useLayoutEffect(() => {
    if (!open) {
      setShift(0);
      return;
    }
    const el = popoverRef.current;
    if (!el) return;
    el.style.right = "0px";
    const rect = el.getBoundingClientRect();
    const overflowLeft = VIEWPORT_MARGIN - rect.left;
    // 右へずらしても右端からははみ出させない。
    const roomRight = document.documentElement.clientWidth - VIEWPORT_MARGIN - rect.right;
    const next = Math.max(0, Math.min(overflowLeft, roomRight));
    el.style.right = "";
    setShift(next);
  }, [open]);

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
        className={triggerClassName ?? styles.trigger}
        title={triggerTitle}
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => {
            const next = !v;
            if (next) onOpen?.();
            return next;
          });
        }}
      >
        {triggerLabel}
      </button>
      {open && (
        <div
          ref={popoverRef}
          className={styles.popover}
          style={shift > 0 ? { right: -shift } : undefined}
          onClick={(e) => e.stopPropagation()}
        >
          <ActionsMenuPopoverList items={items} onPick={pick} />
        </div>
      )}
    </div>
  );
}
