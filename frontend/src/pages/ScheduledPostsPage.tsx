import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, getErrorMessage, ScheduledPost } from "../api/client";
import AppShell from "../components/layout/AppShell";
import ConfirmModal from "../components/common/ConfirmModal";
import Modal from "../components/common/Modal";
import UiIcon from "../components/common/UiIcon";
import PostComposer from "../components/note/PostComposer";
import { useStreamingContext } from "../contexts/StreamingContext";
import { useToast } from "../contexts/ToastContext";
import { acct } from "../lib/format";
import { scheduledFailureMessage } from "../lib/scheduledPost";
import panel from "../components/common/Panel.module.css";
import styles from "./ScheduledPosts.module.css";

const VISIBILITY_ICONS: Record<string, string> = {
  public: "🌐",
  unlisted: "🌙",
  followers_only: "🔒️",
  direct: "✉️",
};

/** 「機能」＞「予約投稿」。投稿待ちの予約と投稿できなかった予約を予約日時の順に並べ、編集
 * （再予約・今すぐ投稿）・今すぐ投稿・削除ができる。 */
export default function ScheduledPostsPage() {
  const { t, i18n } = useTranslation();
  const { showError, showSuccess } = useToast();
  const { refreshScheduledPostCounts, registerScheduledPostsChanged } = useStreamingContext();
  const [items, setItems] = useState<ScheduledPost[] | null>(null);
  const [editing, setEditing] = useState<ScheduledPost | null>(null);
  const [confirm, setConfirm] = useState<{ kind: "publish" | "delete"; post: ScheduledPost } | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    api.scheduledPosts
      .list()
      .then(setItems)
      .catch((e) => showError(getErrorMessage(e)));
  }, [showError]);

  useEffect(() => {
    load();
    // 実行ループの投稿・失敗や、他のタブでの操作を反映する。
    return registerScheduledPostsChanged(load);
  }, [load, registerScheduledPostsChanged]);

  function formatTime(iso: string) {
    return new Date(iso).toLocaleString(i18n.language, { dateStyle: "medium", timeStyle: "short" });
  }

  async function runConfirmed() {
    if (!confirm) return;
    setBusy(true);
    try {
      if (confirm.kind === "publish") {
        // 一覧を読んだ後に別の画面で編集されていれば、その内容で投稿する
        await api.scheduledPosts.publish(confirm.post.id, confirm.post.request, confirm.post);
        showSuccess(t("features:scheduledPosts.published"));
      } else {
        await api.scheduledPosts.delete(confirm.post.id);
        showSuccess(t("features:scheduledPosts.deleted"));
      }
      setConfirm(null);
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setBusy(false);
      refreshScheduledPostCounts();
      load();
    }
  }

  function closeEditor() {
    setEditing(null);
    refreshScheduledPostCounts();
    load();
  }

  const center = (
    <>
      <h2 className={styles.heading}>
        <UiIcon emoji="🕒" /> {t("features:scheduledPosts.title")}
      </h2>
      <p className={styles.description}>{t("features:scheduledPosts.description")}</p>
      {items === null && <p className={panel.message}>{t("common:loading")}</p>}
      {items && items.length === 0 && <p className={panel.message}>{t("features:scheduledPosts.empty")}</p>}
      {items && items.length > 0 && (
        <ul className={styles.list}>
          {items.map((post) => {
            const req = post.request;
            const visibility = req.visibility ?? "public";
            return (
              <li key={post.id} className={`${styles.item} ${post.failureCode ? styles.failed : ""}`}>
                <div className={styles.meta}>
                  <span className={styles.time}>
                    <UiIcon emoji="🕒" /> {formatTime(post.scheduledAt)}
                  </span>
                  <span className={styles.flags}>
                    <UiIcon emoji={VISIBILITY_ICONS[visibility] ?? "🌐"} />
                    {req.deliver_to_fedi !== false && <span className={styles.flag}>Fedi</span>}
                    {req.deliver_to_bsky !== false && <span className={styles.flag}>Bsky</span>}
                  </span>
                </div>
                {post.failureCode && (
                  <p className={styles.failure} role="alert">
                    <UiIcon emoji="⚠️" /> {t("features:scheduledPosts.failed")}
                    {scheduledFailureMessage(post.failureCode)}
                  </p>
                )}
                {post.replyTo && (
                  <p className={styles.reference}>
                    {t("features:scheduledPosts.replyTo", { acct: acct(post.replyTo) })} {post.replyTo.text}
                  </p>
                )}
                {post.quoteOf && (
                  <p className={styles.reference}>
                    {t("features:scheduledPosts.quoteOf", { acct: acct(post.quoteOf) })} {post.quoteOf.text}
                  </p>
                )}
                {req.content_warning && (
                  <p className={styles.cw}>
                    <UiIcon emoji="⚠️" /> {req.content_warning}
                  </p>
                )}
                <p className={styles.text}>{req.text}</p>
                {req.poll && (
                  <p className={styles.poll}>
                    <UiIcon emoji="📊" /> {req.poll.choices.join(" / ")}
                  </p>
                )}
                {post.attachments.length > 0 && (
                  <div className={styles.attachments}>
                    {post.attachments.map((a) =>
                      a.mimeType.startsWith("image/") ? (
                        <img key={a.id} src={a.thumbnailUrl ?? a.url} alt="" className={styles.thumb} />
                      ) : (
                        <span key={a.id} className={styles.fileChip}>
                          {a.mimeType.startsWith("video/") ? "🎞️" : a.mimeType.startsWith("audio/") ? "🎵" : "📎"}
                        </span>
                      ),
                    )}
                  </div>
                )}
                <div className={styles.actions}>
                  <button type="button" className={styles.actionBtn} onClick={() => setEditing(post)}>
                    {t("features:scheduledPosts.edit")}
                  </button>
                  <button
                    type="button"
                    className={styles.actionBtn}
                    onClick={() => setConfirm({ kind: "publish", post })}
                  >
                    {t("features:scheduledPosts.publishNow")}
                  </button>
                  <button
                    type="button"
                    className={`${styles.actionBtn} ${styles.dangerBtn}`}
                    onClick={() => setConfirm({ kind: "delete", post })}
                  >
                    {t("features:scheduledPosts.delete")}
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <Modal open={!!editing} onClose={closeEditor} title={t("features:scheduledPosts.editTitle")}>
        {editing && (
          <PostComposer
            key={editing.id}
            editingScheduled={editing}
            replyTo={editing.replyTo ?? undefined}
            quoteTo={editing.quoteOf ?? undefined}
            autoFocus
            onScheduled={closeEditor}
            onPosted={() => {
              showSuccess(t("features:scheduledPosts.published"));
              closeEditor();
            }}
          />
        )}
      </Modal>

      <ConfirmModal
        open={!!confirm}
        onClose={() => setConfirm(null)}
        title={
          confirm?.kind === "publish"
            ? t("features:scheduledPosts.publishConfirmTitle")
            : t("features:scheduledPosts.deleteConfirmTitle")
        }
        confirmLabel={
          confirm?.kind === "publish" ? t("features:scheduledPosts.publishNow") : t("features:scheduledPosts.delete")
        }
        onConfirm={runConfirmed}
        busy={busy}
      >
        <p>
          {confirm?.kind === "publish"
            ? t("features:scheduledPosts.publishConfirmBody")
            : t("features:scheduledPosts.deleteConfirmBody")}
        </p>
      </ConfirmModal>
    </>
  );

  return <AppShell center={center} />;
}
