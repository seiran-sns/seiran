import { ReactNode, useRef, useState } from "react";
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
}

/**
 * 子要素へのマウスオーバー中、指定されたポストの概要を表示する。
 * 返信先インジケータと通知アイテムで共用する。
 */
export default function NoteHoverPreview({ noteId, children, className }: NoteHoverPreviewProps) {
  const { t } = useTranslation();
  const [target, setTarget] = useState<Note | null>(null);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  const [open, setOpen] = useState(false);
  const [showContent, setShowContent] = useState(false);
  const fetchedRef = useRef(false);
  const timerRef = useRef<number | null>(null);

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
    setOpen(true);
  }

  function onLeave() {
    // 少し遅延させてから閉じる（ポップアップへのカーソル移動を許容）。
    timerRef.current = window.setTimeout(() => setOpen(false), 120);
  }

  return (
    <span
      className={className ? `${styles.wrap} ${className}` : styles.wrap}
      onMouseEnter={onEnter}
      onMouseLeave={onLeave}
    >
      {children}
      {open && (
        <span className={styles.popup}>
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
