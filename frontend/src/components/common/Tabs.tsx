import styles from "./Tabs.module.css";

interface TabsProps {
  tabs: string[];
  active: number;
  onChange: (index: number) => void;
}

/** 右ペイン等で使う横並びタブ。選択状態は呼び出し側が保持する。 */
export default function Tabs({ tabs, active, onChange }: TabsProps) {
  return (
    <div className={styles.tabs}>
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
