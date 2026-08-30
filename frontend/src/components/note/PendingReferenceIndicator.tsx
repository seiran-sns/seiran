import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import styles from "./PendingReferenceIndicator.module.css";

interface PendingReferenceIndicatorProps {
  /** この参照を持つ投稿自身のID（解決対象ではなく、参照元）。 */
  noteId: string;
  kind: "reply" | "quote" | "repost";
  status: "pending" | "gone";
  /** 取り込みに成功した場合、解決先の投稿IDで呼ばれる。 */
  onResolved: (resolvedId: string) => void;
}

/**
 * リプライ/引用/リポストの参照が`pending`（未取り込み）/`gone`（消失）な場合の表示（#234）。
 * `gone`はボタン無しの案内のみ、`pending`は「取り込む」ボタンでその場フェッチを試みる。
 * タイムライン上のNoteCard・ポスト詳細画面のどちらでも使う。
 */
export default function PendingReferenceIndicator({
  noteId,
  kind,
  status,
  onResolved,
}: PendingReferenceIndicatorProps) {
  const { t } = useTranslation();
  const [currentStatus, setCurrentStatus] = useState(status);
  const [importing, setImporting] = useState(false);

  async function handleImport(e: React.MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    if (importing) return;
    setImporting(true);
    try {
      const res = await api.notes.resolveReference(noteId, kind);
      if (res.status === "resolved" && res.postId) {
        onResolved(res.postId);
        return;
      }
      setCurrentStatus(res.status === "gone" ? "gone" : "pending");
    } catch {
      // pendingのまま据え置き、再試行できるようにする
    } finally {
      setImporting(false);
    }
  }

  if (currentStatus === "gone") {
    return (
      <span className={styles.notice} onClick={(e) => e.stopPropagation()}>
        {t("home:noteCard.referenceGoneNotice")}
      </span>
    );
  }

  return (
    <span className={styles.notice} onClick={(e) => e.stopPropagation()}>
      {t("home:noteCard.referencePendingNotice")}
      <button
        type="button"
        className={styles.importButton}
        onClick={handleImport}
        disabled={importing}
      >
        {importing
          ? t("home:noteCard.referenceImporting")
          : t("home:noteCard.referenceImportButton")}
      </button>
    </span>
  );
}
