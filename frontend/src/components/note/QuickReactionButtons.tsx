import { CSSProperties, RefObject, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useIsTopmostCard } from "../../hooks/useIsTopmostCard";
import { isSameReaction, QUICK_REACTION_HEART, QuickReaction } from "../../lib/quickReactions";
import TwemojiEmoji from "../common/TwemojiEmoji";
import noteCardStyles from "./NoteCard.module.css";
import styles from "./QuickReactionButtons.module.css";

/** ポップアップの絵文字の高さ（px）。横長のカスタム絵文字は高さを揃えて横に伸ばす。 */
const POPUP_EMOJI_PX = 20;
/** 横長のカスタム絵文字の最大幅（高さに対する倍率）。リアクション表示欄（`ReactionChips`）と揃える。 */
const POPUP_EMOJI_MAX_ASPECT = 4;

interface ReactionTitleProps {
  content: string;
  active: boolean;
}

function useReactionTitle() {
  const { t } = useTranslation();
  return ({ content, active }: ReactionTitleProps) =>
    t(active ? "home:quickReaction.removeTitle" : "home:quickReaction.reactTitle", { emoji: content });
}

interface QuickHeartButtonProps {
  /** 自分が付けているリアクションの保存値（未リアクションなら undefined）。 */
  myReactionEmoji: string | undefined;
  disabled: boolean;
  /** 幅が足りないときに隠す（`useFitCount`）。 */
  hidden: boolean;
  onToggleReaction: (emoji: string) => void;
}

/**
 * ボタンバーの❤️。押すと❤️で付け、❤️を付けていれば（枠で示す）取り消す。別の絵文字を付けて
 * いれば1投稿1リアクションなので置き換わる。
 */
export function QuickHeartButton({ myReactionEmoji, disabled, hidden, onToggleReaction }: QuickHeartButtonProps) {
  const title = useReactionTitle();
  const active = myReactionEmoji !== undefined && isSameReaction(QUICK_REACTION_HEART, myReactionEmoji);
  return (
    <button
      type="button"
      data-fit-key={QUICK_REACTION_HEART}
      className={`${noteCardStyles.actionBtn} ${active ? noteCardStyles.actionBtnActive : ""}`}
      style={hidden ? { display: "none" } : undefined}
      disabled={disabled}
      onClick={(e) => {
        e.stopPropagation();
        onToggleReaction(active && myReactionEmoji ? myReactionEmoji : QUICK_REACTION_HEART);
      }}
      title={title({ content: QUICK_REACTION_HEART, active })}
    >
      <TwemojiEmoji emoji={QUICK_REACTION_HEART} />
    </button>
  );
}

/** マウスで操作する環境か（ホバーできない＝タッチ環境なら false）。 */
function useCanHover(): boolean {
  const query = "(hover: hover)";
  const [canHover, setCanHover] = useState(
    () => typeof window === "undefined" || !window.matchMedia || window.matchMedia(query).matches
  );
  useEffect(() => {
    if (!window.matchMedia) return;
    const mql = window.matchMedia(query);
    const onChange = () => setCanHover(mql.matches);
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);
  return canHover;
}

interface QuickReactionPopupProps {
  /** よく使う絵文字（❤️を除く、よく使う順）。 */
  reactions: QuickReaction[];
  /** ボタンバー。タッチ環境では、これを含むカードが「上端が見えているカードのうちいちばん上」のときだけ出す。 */
  anchorRef: RefObject<HTMLElement>;
  myReactionEmoji: string | undefined;
  disabled: boolean;
  onToggleReaction: (emoji: string) => void;
}

/** カスタム絵文字の表示幅。実寸の縦横比で高さを揃え、横長すぎるものは最大幅で縮める。 */
function customEmojiStyle(q: QuickReaction): CSSProperties {
  const aspect = q.width && q.height ? q.width / q.height : 1;
  return { width: `${POPUP_EMOJI_PX * Math.min(aspect, POPUP_EMOJI_MAX_ASPECT)}px`, height: `${POPUP_EMOJI_PX}px` };
}

/**
 * ボタンバーのすぐ下に出す、よく使う絵文字を1行に並べたポップアップ。マウス環境ではカードに
 * ホバーしている間（キーボード操作中はカード内にフォーカスがある間）、タッチ環境では画面内で
 * 上端が見えているカードのうちいちばん上の1枚にだけ出す（全カードに出すと、どれを押すか目が迷うため）。
 *
 * 横幅に入るだけ並べる: 1行の高さで折り返させ、2行目以降に回った絵文字を `overflow: hidden` で
 * 見せない（途中で切れた絵文字を出さないため、はみ出しではなく折り返しで落とす）。
 */
export function QuickReactionPopup({
  reactions,
  anchorRef,
  myReactionEmoji,
  disabled,
  onToggleReaction,
}: QuickReactionPopupProps) {
  const title = useReactionTitle();
  const canHover = useCanHover();
  const isTopmost = useIsTopmostCard(anchorRef, !canHover && reactions.length > 0);
  if (reactions.length === 0) return null;

  return (
    <div
      className={`${styles.popup} ${canHover ? noteCardStyles.quickPopupOnHover : isTopmost ? styles.popupShown : ""}`}
      role="toolbar"
    >
      <div className={styles.row}>
        {reactions.map((q) => {
          const active = myReactionEmoji !== undefined && isSameReaction(q.content, myReactionEmoji);
          return (
            <button
              key={q.content}
              type="button"
              className={`${styles.item} ${active ? styles.itemActive : ""}`}
              disabled={disabled}
              onClick={(e) => {
                e.stopPropagation();
                onToggleReaction(active && myReactionEmoji ? myReactionEmoji : q.content);
              }}
              title={title({ content: q.content, active })}
            >
              {q.imageUrl ? (
                <img
                  className={styles.customEmoji}
                  style={customEmojiStyle(q)}
                  src={q.imageUrl}
                  alt={q.content}
                  draggable={false}
                  loading="lazy"
                />
              ) : (
                <TwemojiEmoji emoji={q.content} className={styles.unicodeEmoji} />
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}
