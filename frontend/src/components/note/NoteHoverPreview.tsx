import { ReactNode, useRef, useState, type CSSProperties } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, Note } from "../../api/client";
import Avatar from "./Avatar";
import EmojiText from "./EmojiText";
import TwemojiEmoji from "../common/TwemojiEmoji";
import { acct, displayName } from "../../lib/format";
import styles from "./NoteHoverPreview.module.css";

interface NoteHoverPreviewProps {
  noteId: string;
  children: ReactNode;
  className?: string;
  /** ポップアップの表示位置。"bottom"（既定）は子要素の直下、"left"は子要素の左側。
   * "left"は右ペインの通知アイテムのように、真下に出すとポップアップへマウスが乗って
   * 消えなくなり下のアイテムが押せなくなる場面で使う。 */
  side?: "bottom" | "left";
}

/**
 * 子要素へのマウスオーバー中、指定されたポストの概要を表示する。
 * 返信先インジケータと通知アイテムで共用する。
 */
export default function NoteHoverPreview({ noteId, children, className, side = "bottom" }: NoteHoverPreviewProps) {
  const { t } = useTranslation();
  const [target, setTarget] = useState<Note | null>(null);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  const [open, setOpen] = useState(false);
  const [showContent, setShowContent] = useState(false);
  const [fixedStyle, setFixedStyle] = useState<CSSProperties | null>(null);
  const fetchedRef = useRef(false);
  const timerRef = useRef<number | null>(null);
  const wrapRef = useRef<HTMLSpanElement>(null);

  function ensureFetched() {
    if (fetchedRef.current) return;
    fetchedRef.current = true;
    setLoading(true);
    api.notes
      .get(noteId)
      .then(setTarget)
      .catch(() => setFailed(true))
      .finally(() => setLoading(false));
  }

  function onEnter() {
    ensureFetched();
    if (timerRef.current) window.clearTimeout(timerRef.current);
    // side="left"は右ペインの通知アイテムで使う想定。右ペインは独自に縦スクロールする
    // コンテナ（AppShellの.rightScroll）であり、CSSの仕様上「縦だけauto、横はvisible」
    // という指定はできない（片方がvisibleでない場合は両方autoに揃えられる）ため、
    // CSSのみの絶対配置だとポップアップの左側がそのスクロール境界でクリップされて
    // ほぼ見えなくなる（実機確認済みの回帰）。position: fixedへ切り替え、
    // トリガー要素の実測座標を使って画面基準で配置することでこれを回避する。
    if (side === "left" && wrapRef.current) {
      const rect = wrapRef.current.getBoundingClientRect();
      setFixedStyle({
        position: "fixed",
        top: rect.top + rect.height / 2,
        right: window.innerWidth - rect.left + 10,
        left: "auto",
        transform: "translateY(-50%)",
      });
    }
    setOpen(true);
  }

  function onLeave() {
    // 少し遅延させてから閉じる（ポップアップへのカーソル移動を許容）。
    timerRef.current = window.setTimeout(() => setOpen(false), 120);
  }

  return (
    <span
      ref={wrapRef}
      className={className ? `${styles.wrap} ${className}` : styles.wrap}
      onMouseEnter={onEnter}
      onMouseLeave={onLeave}
    >
      {children}
      {open && (
        <span
          className={side === "left" ? `${styles.popup} ${styles.popupLeft}` : styles.popup}
          style={side === "left" && fixedStyle ? fixedStyle : undefined}
        >
          {loading && <span className={styles.dim}>{t("common:loading")}</span>}
          {failed && <span className={styles.dim}>{t("home:replyIndicator.fetchFailed")}</span>}
          {target && (
            <Link to={`/notes/${target.id}`} className={styles.card} onClick={(e) => e.stopPropagation()}>
              <span className={styles.head}>
                <Avatar
                  url={target.user.avatarUrl}
                  name={target.user.displayName || target.user.username}
                  size={26}
                />
                <span className={styles.names}>
                  <span className={styles.name}>
                  <EmojiText text={displayName(target)} emojis={target.emojis} />
                </span>
                  <span className={styles.acctText}>{acct(target)}</span>
                </span>
              </span>
              {target.contentWarning && (
                <span className={styles.cw}>
                  <span className={styles.cwText}>
                    <TwemojiEmoji emoji="⚠️" /> <EmojiText text={target.contentWarning} emojis={target.emojis} />
                  </span>
                  <button
                    type="button"
                    onClick={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      setShowContent((shown) => !shown);
                    }}
                  >
                    {showContent
                      ? t("home:noteCard.hideContent")
                      : t("home:noteCard.showContent")}
                  </button>
                </span>
              )}
              {(!target.contentWarning || showContent) && (
                <span className={styles.text}>
                  <EmojiText text={target.text} emojis={target.emojis} />
                </span>
              )}
            </Link>
          )}
        </span>
      )}
    </span>
  );
}
