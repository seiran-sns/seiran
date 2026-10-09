import { useEffect, useRef } from "react";
import styles from "./Tabs.module.css";

interface TabsProps {
  tabs: string[];
  active: number;
  onChange: (index: number) => void;
  /**
   * trueの場合、position: sticky で画面上部（直上のsticky headerの下端）に張り付ける。
   * 未指定時は従来通り非stickyのまま（他の呼び出し元の見た目に影響しない）。
   */
  sticky?: boolean;
  /** sticky指定時の上端オフセット（px）。直上のheader要素の実高さを渡す想定。 */
  top?: number;
  /**
   * trueの場合、タブが多くて幅に収まらないときに横スクロールさせる（各タブは均等割りせず
   * 内容幅）。選択中のタブはコンテナ内で中央寄せに追従する。
   */
  scrollable?: boolean;
}

/** 右ペイン等で使う横並びタブ。選択状態は呼び出し側が保持する。 */
export default function Tabs({ tabs, active, onChange, sticky, top, scrollable }: TabsProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  // 選択中のタブが見えるよう、タブコンテナだけを横移動する（scrollIntoViewはwindowも縦に動かし
  // スクロール位置の復元を壊すため使わない）。
  useEffect(() => {
    if (!scrollable) return;
    const container = containerRef.current;
    const activeEl = container?.children[active] as HTMLElement | undefined;
    if (!container || !activeEl) return;
    const left = activeEl.offsetLeft - (container.clientWidth - activeEl.offsetWidth) / 2;
    container.scrollTo({ left: Math.max(0, left), behavior: "smooth" });
  }, [active, scrollable]);

  return (
    <div
      ref={containerRef}
      className={`${styles.tabs} ${sticky ? styles.sticky : ""} ${scrollable ? styles.scrollable : ""}`}
      style={sticky ? { top } : undefined}
    >
      {tabs.map((label, i) => (
        <button
          key={i}
          className={`${styles.tab} ${i === active ? styles.active : ""}`}
          onClick={() => onChange(i)}
        >
          {label}
        </button>
      ))}
    </div>
  );
}
