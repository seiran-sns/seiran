import { CSSProperties, PointerEvent as ReactPointerEvent, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  FLICK_DIRECTIONS,
  FlickDirection,
  flickDirection,
  isSameReaction,
  QuickReaction,
} from "../../lib/quickReactions";
import TwemojiEmoji from "../common/TwemojiEmoji";
import noteCardStyles from "./NoteCard.module.css";
import styles from "./QuickReactionButtons.module.css";

interface QuickReactionButtonsProps {
  /** 先頭が❤️、続いてよく使う絵文字。上限に満たない分は `null`（空き枠）。 */
  slots: (QuickReaction | null)[];
  /** 幅に収まる個数（`useFitCount`）。これ以降は `display: none` にする。 */
  visibleCount: number;
  /** 自分が付けているリアクションの保存値（未リアクションなら undefined）。 */
  myReactionEmoji: string | undefined;
  disabled: boolean;
  onToggleReaction: (emoji: string) => void;
}

function ReactionGlyph({ reaction, className }: { reaction: QuickReaction; className: string }) {
  return reaction.imageUrl ? (
    <img className={className} src={reaction.imageUrl} alt={reaction.content} draggable={false} loading="lazy" />
  ) : (
    <TwemojiEmoji emoji={reaction.content} className={className} />
  );
}

/**
 * NoteCard のアクション行に並べるクイックリアクションボタン（❤️＋よく使う絵文字の枠）。
 * 親の flex 行の直接の子として並ぶよう Fragment で返す（`useFitCount` が子要素を測るため）。
 *
 * - よく使う絵文字のボタンは、マウス環境ではカードにホバー中だけ、タッチ環境では出さない
 *   （CSS、`NoteCard.module.css` の `quickFrequent`）。
 * - タッチ（ペン含む）で❤️に指を置くと、よく使う絵文字が❤️の左・上・右にせり出し、
 *   指を滑らせた方向の絵文字を付ける（フリック入力と同じ操作）。指を置いた時点で出すのは、
 *   長押しを待たせると操作の存在に気付けないため。
 */
export default function QuickReactionButtons({
  slots,
  visibleCount,
  myReactionEmoji,
  disabled,
  onToggleReaction,
}: QuickReactionButtonsProps) {
  const { t } = useTranslation();

  function isActive(q: QuickReaction) {
    return myReactionEmoji !== undefined && isSameReaction(q.content, myReactionEmoji);
  }

  /** 青枠のボタンは自分のリアクションを取り消し、それ以外はその絵文字で付ける（付け替え）。 */
  function press(q: QuickReaction) {
    onToggleReaction(isActive(q) && myReactionEmoji ? myReactionEmoji : q.content);
  }

  function title(q: QuickReaction) {
    return t(isActive(q) ? "home:quickReaction.removeTitle" : "home:quickReaction.reactTitle", {
      emoji: q.content,
    });
  }

  const [heart, ...frequentSlots] = slots;
  const frequent = frequentSlots.filter((q): q is QuickReaction => q !== null);

  return (
    <>
      {heart && (
        <FlickHeart
          heart={heart}
          frequent={frequent}
          hidden={visibleCount < 1}
          disabled={disabled}
          isActive={isActive}
          press={press}
          title={title(heart)}
        />
      )}
      {frequentSlots.map((q, j) => {
        const i = j + 1;
        const hiddenStyle: CSSProperties | undefined = i < visibleCount ? undefined : { display: "none" };
        if (!q) {
          return (
            <span
              key={`slot:${i}`}
              data-fit-key={`slot:${i}`}
              className={`${noteCardStyles.actionBtn} ${noteCardStyles.quickFrequent} ${noteCardStyles.quickSlotEmpty}`}
              style={hiddenStyle}
              aria-hidden="true"
            >
              <span className={noteCardStyles.quickSlotBox} />
            </span>
          );
        }
        return (
          <button
            key={q.content}
            type="button"
            data-fit-key={q.content}
            className={`${noteCardStyles.actionBtn} ${noteCardStyles.quickFrequent} ${isActive(q) ? noteCardStyles.actionBtnActive : ""}`}
            style={hiddenStyle}
            disabled={disabled}
            onClick={(e) => {
              e.stopPropagation();
              press(q);
            }}
            title={title(q)}
          >
            <ReactionGlyph reaction={q} className={noteCardStyles.quickReactionImg} />
          </button>
        );
      })}
    </>
  );
}

interface FlickHeartProps {
  heart: QuickReaction;
  frequent: QuickReaction[];
  hidden: boolean;
  disabled: boolean;
  isActive: (q: QuickReaction) => boolean;
  press: (q: QuickReaction) => void;
  title: string;
}

function FlickHeart({ heart, frequent, hidden, disabled, isActive, press, title }: FlickHeartProps) {
  const [open, setOpen] = useState(false);
  const [direction, setDirection] = useState<FlickDirection | "none" | null>("none");
  const start = useRef<{ x: number; y: number; pointerId: number } | null>(null);
  // タッチで操作したあとに続けて発火する click を、二重に処理しないよう読み捨てる。
  const suppressClick = useRef(false);

  const byDirection = new Map<FlickDirection, QuickReaction>(
    frequent.map((q, i) => [FLICK_DIRECTIONS[i], q])
  );

  function close() {
    start.current = null;
    setOpen(false);
    setDirection("none");
  }

  function onPointerDown(e: ReactPointerEvent<HTMLButtonElement>) {
    if (e.pointerType === "mouse" || disabled || e.button !== 0) {
      suppressClick.current = false;
      return;
    }
    suppressClick.current = true;
    start.current = { x: e.clientX, y: e.clientY, pointerId: e.pointerId };
    e.currentTarget.setPointerCapture(e.pointerId);
    setDirection("none");
    setOpen(frequent.length > 0);
  }

  function onPointerMove(e: ReactPointerEvent<HTMLButtonElement>) {
    const s = start.current;
    if (!s || s.pointerId !== e.pointerId) return;
    setDirection(flickDirection(e.clientX - s.x, e.clientY - s.y));
  }

  function onPointerUp(e: ReactPointerEvent<HTMLButtonElement>) {
    const s = start.current;
    if (!s || s.pointerId !== e.pointerId) return;
    e.stopPropagation();
    // 最後の move から指を離すまでに動いた分も反映する。
    const dir = flickDirection(e.clientX - s.x, e.clientY - s.y);
    close();
    if (dir === "none") press(heart);
    else if (dir !== null) {
      const q = byDirection.get(dir);
      if (q) press(q);
    }
  }

  return (
    <span
      data-fit-key={heart.content}
      className={styles.heartWrap}
      style={hidden ? { display: "none" } : undefined}
    >
      <button
        type="button"
        className={`${noteCardStyles.actionBtn} ${styles.heartBtn} ${isActive(heart) ? noteCardStyles.actionBtnActive : ""}`}
        disabled={disabled}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={close}
        onContextMenu={(e) => e.preventDefault()}
        // ホームのタブ切り替えスワイプ（`useSwipe`、タッチイベント）に左右のフリックを拾わせない。
        onTouchStart={(e) => e.stopPropagation()}
        onTouchEnd={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          if (suppressClick.current) {
            suppressClick.current = false;
            return;
          }
          press(heart);
        }}
        title={title}
      >
        <ReactionGlyph reaction={heart} className={noteCardStyles.quickReactionImg} />
      </button>
      {open && (
        <span className={styles.flickLayer} aria-hidden="true">
          {FLICK_DIRECTIONS.map((dir) => {
            const q = byDirection.get(dir);
            if (!q) return null;
            return (
              <span
                key={dir}
                className={`${styles.flickItem} ${styles[dir]} ${direction === dir ? styles.flickItemSelected : ""} ${isActive(q) ? styles.flickItemActive : ""}`}
              >
                <ReactionGlyph reaction={q} className={styles.flickGlyph} />
              </span>
            );
          })}
        </span>
      )}
    </span>
  );
}
