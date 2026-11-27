import type { ReactNode } from "react";
import styles from "./SwitchField.module.css";

interface SwitchFieldProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  /** スイッチの右に置く項目名。 */
  children: ReactNode;
}

/** 切り替えた時点で保存・反映される設定のためのスライドスイッチ（チェックボックスに見せると
 * 「保存」ボタンを探させてしまう）。中身は `role="switch"` のチェックボックス。 */
export default function SwitchField({ checked, onChange, disabled, children }: SwitchFieldProps) {
  return (
    <label className={styles.field}>
      <input
        type="checkbox"
        role="switch"
        className={styles.switch}
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      {children}
    </label>
  );
}
