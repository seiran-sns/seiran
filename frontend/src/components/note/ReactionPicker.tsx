import { lazy, Suspense, useState } from "react";
import { useTranslation } from "react-i18next";
import Modal from "../common/Modal";
import noteCardStyles from "./NoteCard.module.css";

// Unicode 絵文字データセット（unicode-emoji-json、非圧縮で数百KB）を含むため、ピッカーを
// 実際に開くまでロードしない（バンドルサイズ対策）。
const EmojiPickerPanel = lazy(() => import("./EmojiPickerPanel"));

interface ReactionPickerProps {
  /** 絵文字が選択された時に呼ばれる（Unicode絵文字文字列 or `:shortcode:`）。 */
  onPick: (emoji: string) => void;
  /** リアクション操作中は true。トリガーボタンを無効化する。 */
  disabled?: boolean;
  /** 外部（`ActionsMenu` の「リアクション」項目等）から開閉を制御したい場合に指定する。 */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

/** 投稿に絵文字リアクションを付けるためのトリガーボタン＋ピッカー（Modal 内に検索・タブ・グリッド）。 */
export default function ReactionPicker({ onPick, disabled, open: controlledOpen, onOpenChange }: ReactionPickerProps) {
  const { t } = useTranslation();
  const [uncontrolledOpen, setUncontrolledOpen] = useState(false);
  const open = controlledOpen ?? uncontrolledOpen;
  const setOpen = onOpenChange ?? setUncontrolledOpen;

  function pick(emoji: string) {
    setOpen(false);
    onPick(emoji);
  }

  return (
    <>
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
      <Modal open={open} onClose={() => setOpen(false)} title={t("home:reactionPicker.addReactionTitle")}>
        {open && (
          <Suspense fallback={<p>{t("common:loading")}</p>}>
            <EmojiPickerPanel onPick={pick} />
          </Suspense>
        )}
      </Modal>
    </>
  );
}
