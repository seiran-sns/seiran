import { useCallback, useEffect, useMemo, useState } from "react";
import { AdminReport, api, getErrorMessage, ReportComment } from "../../api/client";
import { useToast } from "../../contexts/ToastContext";
import { findReportReasonLabel } from "../report/reportReasons";
import styles from "../../pages/Admin.module.css";

export default function ReportsPanel() {
  const { showError } = useToast();
  const [reports, setReports] = useState<AdminReport[]>([]);
  const [comments, setComments] = useState<Record<string, ReportComment[]>>({});
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [statusFilter, setStatusFilter] = useState<"open" | "closed">("open");

  const reload = useCallback(async () => {
    try { setReports(await api.admin.listReports()); }
    catch (e) { showError(getErrorMessage(e)); }
  }, [showError]);
  useEffect(() => { void reload(); }, [reload]);

  const filtered = useMemo(
    () => reports.filter((r) => r.status === statusFilter),
    [reports, statusFilter],
  );

  async function loadComments(id: string) {
    try {
      const loaded = await api.admin.listReportComments(id);
      setComments((v) => ({ ...v, [id]: loaded }));
    }
    catch (e) { showError(getErrorMessage(e)); }
  }
  async function addComment(id: string) {
    const body = drafts[id]?.trim();
    if (!body) return;
    try {
      const added = await api.admin.addReportComment(id, body);
      setComments((v) => ({ ...v, [id]: [...(v[id] ?? []), added] }));
      setDrafts((v) => ({ ...v, [id]: "" }));
    } catch (e) { showError(getErrorMessage(e)); }
  }
  async function act(action: () => Promise<void>) {
    try { await action(); await reload(); }
    catch (e) { showError(getErrorMessage(e)); }
  }

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>通報</h2>
      <div className={styles.authStatus}>
        <button
          className={statusFilter === "open" ? styles.btn : styles.btnGhost}
          onClick={() => setStatusFilter("open")}
        >
          未処理
        </button>
        <button
          className={statusFilter === "closed" ? styles.btn : styles.btnGhost}
          onClick={() => setStatusFilter("closed")}
        >
          処理済み
        </button>
      </div>
      {filtered.length === 0 && <p>{statusFilter === "open" ? "未処理の通報はありません。" : "処理済みの通報はありません。"}</p>}
      {filtered.map((r) => (
        <section className={styles.card} key={r.id}>
          <div className={styles.row}>
            <div className={styles.grow}>
              <div className={styles.primaryText}>⚠️ {r.subject} {r.subject_type === "post" ? "の投稿" : "（ユーザー）"}</div>
              <div className={styles.subText}>通報者: {r.reporter} / 理由: {findReportReasonLabel(r.reason_type)} / 対象: {r.destination === "local" ? "ローカル" : r.remote_host}</div>
              <div className={styles.subText}>{new Date(r.created_at).toLocaleString()} / {r.status}</div>
              {r.reason_text && <p>{r.reason_text}</p>}
              <div className={styles.authStatus}>
                <a href={`/@${r.subject}`} target="_blank" rel="noreferrer">対象ユーザーを表示</a>
                {r.subject_post_id && <a href={`/notes/${r.subject_post_id}`} target="_blank" rel="noreferrer">対象投稿を表示</a>}
              </div>
            </div>
          </div>
          <div className={styles.authStatus}>
            {r.status === "open" && <button className={styles.btnGhost} onClick={() => act(() => api.admin.closeReport(r.id))}>クローズ</button>}
            {r.destination === "remote" && !r.forwarded_at && <button className={styles.btn} onClick={() => act(() => api.admin.forwardReport(r.id))}>転送</button>}
            {r.subject_post_id && <button className={styles.btnDanger} onClick={() => act(() => api.admin.deleteReportedPost(r.id))}>投稿削除</button>}
            <button className={styles.btnDanger} onClick={() => act(() => api.admin.suspendReportedUser(r.id))}>ユーザー凍結</button>
            <button className={styles.btnGhost} onClick={() => loadComments(r.id)}>内部コメント</button>
          </div>
          {comments[r.id] && (
            <div>
              {comments[r.id].map((c) => <p key={c.id}><strong>{c.author}</strong>: {c.body}</p>)}
              <textarea className={styles.input} value={drafts[r.id] ?? ""} maxLength={2000}
                onChange={(e) => setDrafts((v) => ({ ...v, [r.id]: e.target.value }))} />
              <button className={styles.btn} onClick={() => addComment(r.id)}>コメント追加</button>
            </div>
          )}
        </section>
      ))}
    </div>
  );
}
