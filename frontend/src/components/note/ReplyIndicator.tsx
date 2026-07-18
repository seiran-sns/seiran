import { useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, Note } from "../../api/client";
import { acct, displayName } from "../../lib/format";
import styles from "./ReplyIndicator.module.css";

/**
 * リプライであることを示す ↩️ インジケータ（issue #20）。
 * マウスオーバーで返信先ポストをフェッチしてポップアップ表示する。
 * タイムライン・詳細画面の両方で使用する。
 */
export default function ReplyIndicator({ replyId }: { replyId: string }) {
  const { t } = useTranslation();
  const [target, setTarget] = useState<Note | null>(null);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  const [open, setOpen] = useState(false);
  const fetchedRef = useRef(false);
  const timerRef = useRef<number | null>(null);

  function ensureFetched() {
    if (fetchedRef.current) return;
    fetchedRef.current = true;
    setLoading(true);
    api.notes
      .get(replyId)
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
    <span className={styles.wrap} onMouseEnter={onEnter} onMouseLeave={onLeave}>
      <Link
        to={`/notes/${replyId}`}
        className={styles.indicator}
        onClick={(e) => e.stopPropagation()}
        title={t("home:replyIndicator.goToOriginalTitle")}
      >
        <span aria-hidden>↩️</span> {t("home:replyIndicator.replyLabel")}
      </Link>

      {open && (
        <span className={styles.popup} onMouseEnter={onEnter} onMouseLeave={onLeave}>
          {loading && <span className={styles.dim}>{t("common:loading")}</span>}
          {failed && <span className={styles.dim}>{t("home:replyIndicator.fetchFailed")}</span>}
          {target && (
            <Link to={`/notes/${target.id}`} className={styles.card} onClick={(e) => e.stopPropagation()}>
              <span className={styles.head}>
                <span className={styles.avatar}>
                  {(target.user.displayName || target.user.username)[0]?.toUpperCase() ?? "?"}
                </span>
                <span className={styles.names}>
                  <span className={styles.name}>{displayName(target)}</span>
                  <span className={styles.acctText}>{acct(target)}</span>
                </span>
              </span>
              <span className={styles.text}>{target.text}</span>
            </Link>
          )}
        </span>
      )}
    </span>
  );
}
