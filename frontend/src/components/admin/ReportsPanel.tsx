import { useEffect, useState } from "react";
import { AdminReport, api, getErrorMessage, ReportComment } from "../../api/client";
import { useToast } from "../../contexts/ToastContext";
import styles from "../../pages/Admin.module.css";

export default function ReportsPanel() {
  const { showError } = useToast();
  const [reports, setReports] = useState<AdminReport[]>([]);
  const [comments, setComments] = useState<Record<string, ReportComment[]>>({});
  const [drafts, setDrafts] = useState<Record<string, string>>({});

  async function reload() {
    try { setReports(await api.admin.listReports()); }
    catch (e) { showError(getErrorMessage(e)); }
  }
  useEffect(() => { void reload(); }, []);

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
      {reports.length === 0 && <p>通報はありません。</p>}
      {reports.map((r) => (
        <section className={styles.card} key={r.id}>
          <div className={styles.row}>
            <div className={styles.grow}>
              <div className={styles.primaryText}>⚠️ {r.subject} {r.subject_type === "post" ? "の投稿" : "（ユーザー）"}</div>
              <div className={styles.subText}>通報者: {r.reporter} / 理由: {r.reason_type} / 送信先: {r.destination === "local" ? "ローカル" : r.remote_host}</div>
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
