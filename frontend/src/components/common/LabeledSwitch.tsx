import styles from "./LabeledSwitch.module.css";

interface LabeledSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** オフ側（左）・オン側（右）のラベル。スイッチの中に書くので、状態が文面なしで分かる。 */
  offLabel: string;
  onLabel: string;
  /** スクリーンリーダー向けの対象名（何についてのスイッチか）。 */
  ariaLabel: string;
  disabled?: boolean;
}

/** 左右のラベルをスイッチの中に持つスライドスイッチ。現在の状態のラベルがつまみの上に載る。 */
export default function LabeledSwitch({
  checked,
  onChange,
  offLabel,
  onLabel,
  ariaLabel,
  disabled,
}: LabeledSwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      className={styles.switch}
      onClick={() => onChange(!checked)}
    >
      <span className={`${styles.thumb} ${checked ? styles.thumbOn : ""}`} aria-hidden="true" />
      <span className={`${styles.label} ${!checked ? styles.labelActive : ""}`}>{offLabel}</span>
      <span className={`${styles.label} ${checked ? styles.labelActive : ""}`}>{onLabel}</span>
    </button>
  );
}
