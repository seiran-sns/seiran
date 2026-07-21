import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { isValidReactionEmoji } from "../../lib/reaction";
import noteCardStyles from "./NoteCard.module.css";
import styles from "./ReactionPicker.module.css";

const QUICK_EMOJIS = ["👍", "❤️", "😂", "😮", "😢", "🎉"];
const MAX_CONTENT_LEN = 32;

interface ReactionPickerProps {
  /** 絵文字が選択された時に呼ばれる（クイック選択・自由入力の両方から）。 */
  onPick: (emoji: string) => void;
  /** リアクション操作中は true。トリガーボタンを無効化する。 */
  disabled?: boolean;
  /** 外部（`ActionsMenu` の「リアクション」項目等）から開閉を制御したい場合に指定する。 */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

/** 投稿に絵文字リアクションを付けるためのトリガーボタン＋ポップオーバー。 */
export default function ReactionPicker({ onPick, disabled, open: controlledOpen, onOpenChange }: ReactionPickerProps) {
  const { t } = useTranslation();
  const [uncontrolledOpen, setUncontrolledOpen] = useState(false);
  const open = controlledOpen ?? uncontrolledOpen;
  const setOpen = onOpenChange ?? setUncontrolledOpen;
  const [customInput, setCustomInput] = useState("");
  const [customError, setCustomError] = useState(false);
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

  function pick(emoji: string) {
    setOpen(false);
    setCustomInput("");
    setCustomError(false);
    onPick(emoji);
  }

  function handleCustomSubmit(e: React.FormEvent) {
    e.preventDefault();
    e.stopPropagation();
    const trimmed = customInput.trim();
    if (!trimmed || trimmed.length > MAX_CONTENT_LEN || !isValidReactionEmoji(trimmed)) {
      setCustomError(true);
      return;
    }
    pick(trimmed);
  }

  return (
    <div className={styles.wrap} ref={wrapRef}>
      <button
        type="button"
        className={noteCardStyles.actionBtn}
        disabled={disabled}
        onClick={(e) => {
          e.stopPropagation();
          setOpen(!open);
        }}
        title={t("home:reactionPicker.addReactionTitle")}
      >
        🙂 {t("home:reactionPicker.addReactionButton")}
      </button>
      {open && (
        <div className={styles.popover} onClick={(e) => e.stopPropagation()}>
          <div className={styles.quickRow}>
            {QUICK_EMOJIS.map((emoji) => (
              <button
                key={emoji}
                type="button"
                className={styles.quickBtn}
                onClick={() => pick(emoji)}
              >
                {emoji}
              </button>
            ))}
          </div>
          <form className={styles.customRow} onSubmit={handleCustomSubmit}>
            <input
              type="text"
              className={styles.customInput}
              placeholder={t("home:reactionPicker.customInputPlaceholder")}
              value={customInput}
              maxLength={MAX_CONTENT_LEN}
              onChange={(e) => {
                setCustomInput(e.target.value);
                setCustomError(false);
              }}
            />
            <button
              type="submit"
              className={styles.customSubmit}
              disabled={!customInput.trim() || !isValidReactionEmoji(customInput)}
            >
              {t("common:add")}
            </button>
          </form>
          {customError && (
            <p className={styles.customError}>{t("home:reactionPicker.customInputError")}</p>
          )}
        </div>
      )}
    </div>
  );
}
