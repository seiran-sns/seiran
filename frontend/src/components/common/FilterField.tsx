import { ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./FilterField.module.css";

/** 絞り込み欄を横に並べる行。 */
export function FilterBar({ children }: { children: ReactNode }) {
  return <div className={styles.bar}>{children}</div>;
}

/** 表示欄に並べるタグ1つ。`label` は表示名（絵文字展開込み）等の要素でもよいので、識別は `key` で行う。 */
export interface FilterTag {
  key: string;
  label: ReactNode;
}

interface FilterFieldProps {
  label: string;
  placeholder: string;
  tags: FilterTag[];
  /** 指定を全部外す。タグがあるときだけ✕ボタンを出す。 */
  onClear: () => void;
  children: ReactNode;
}

/** テキストボックス風の表示欄。クリックで下にポップオーバー（選択UI）を開き、選択内容はタグ状に並べる。
 * はみ出した分は右端の「…」で省略する。 */
export default function FilterField({ label, placeholder, tags, onClear, children }: FilterFieldProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const tagsRef = useRef<HTMLSpanElement>(null);
  const [overflowing, setOverflowing] = useState(false);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  useLayoutEffect(() => {
    const el = tagsRef.current;
    if (!el) return;
    const measure = () => setOverflowing(el.scrollWidth > el.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [tags]);

  return (
    <div className={styles.field} ref={wrapRef}>
      <button
        type="button"
        className={styles.box}
        aria-label={label}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        {tags.length === 0 ? (
          <span className={styles.placeholder}>{placeholder}</span>
        ) : (
          <>
            <span className={styles.tags} ref={tagsRef}>
              {tags.map((tag) => (
                <span key={tag.key} className={styles.tag}>
                  {tag.label}
                </span>
              ))}
            </span>
            {overflowing && <span className={styles.ellipsis}>…</span>}
          </>
        )}
      </button>
      {tags.length > 0 && (
        <button type="button" className={styles.clear} aria-label={t("common:filterField.clear")} onClick={onClear}>
          ✕
        </button>
      )}
      {open && <div className={styles.popover}>{children}</div>}
    </div>
  );
}


export interface FilterCheckOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface FilterCheckListProps {
  options: FilterCheckOption[];
  selected: string[];
  onToggle: (value: string) => void;
}

/** 複数選択のチェックボックス付きリスト（`FilterField` のポップオーバー内に置く）。 */
export function FilterCheckList({ options, selected, onToggle }: FilterCheckListProps) {
  return (
    <ul className={styles.checkList}>
      {options.map((o) => (
        <li key={o.value}>
          <label className={o.disabled ? `${styles.checkItem} ${styles.checkItemDisabled}` : styles.checkItem}>
            <input
              type="checkbox"
              checked={selected.includes(o.value)}
              disabled={o.disabled}
              onChange={() => onToggle(o.value)}
            />
            {o.label}
          </label>
        </li>
      ))}
    </ul>
  );
}
