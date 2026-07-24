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
}

/** 右ペイン等で使う横並びタブ。選択状態は呼び出し側が保持する。 */
export default function Tabs({ tabs, active, onChange, sticky, top }: TabsProps) {
  return (
    <div
      className={`${styles.tabs} ${sticky ? styles.sticky : ""}`}
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
