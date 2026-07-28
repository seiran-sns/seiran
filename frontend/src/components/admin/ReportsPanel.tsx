import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AdminReport,
  api,
  getErrorMessage,
  ReportComment,
} from "../../api/client";
import { useToast } from "../../contexts/ToastContext";
import { findReportReasonLabel } from "../report/reportReasons";
import styles from "../../pages/Admin.module.css";

export default function ReportsPanel() {
  const { t } = useTranslation();
  const { showError } = useToast();
  const [reports, setReports] = useState<AdminReport[]>([]);
  const [comments, setComments] = useState<Record<string, ReportComment[]>>({});
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [statusFilter, setStatusFilter] = useState<"open" | "closed">("open");

  async function reload() {
    try {
      setReports(await api.admin.listReports());
    } catch (e) {
      showError(getErrorMessage(e));
    }
  }
  useEffect(() => {
    void reload();
  }, []);

  const filtered = useMemo(
    () => reports.filter((r) => r.status === statusFilter),
    [reports, statusFilter],
  );

  async function loadComments(id: string) {
    try {
      const loaded = await api.admin.listReportComments(id);
      setComments((v) => ({ ...v, [id]: loaded }));
    } catch (e) {
      showError(getErrorMessage(e));
    }
  }
  async function addComment(id: string) {
    const body = drafts[id]?.trim();
    if (!body) return;
    try {
      const added = await api.admin.addReportComment(id, body);
      setComments((v) => ({ ...v, [id]: [...(v[id] ?? []), added] }));
      setDrafts((v) => ({ ...v, [id]: "" }));
    } catch (e) {
      showError(getErrorMessage(e));
    }
  }
  async function act(action: () => Promise<void>) {
    try {
      await action();
      await reload();
    } catch (e) {
      showError(getErrorMessage(e));
    }
  }

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:reports.title")}</h2>
      <div className={styles.authStatus}>
        <button
          className={statusFilter === "open" ? styles.btn : styles.btnGhost}
          onClick={() => setStatusFilter("open")}
        >
          {t("admin:reports.openTab")}
        </button>
        <button
          className={statusFilter === "closed" ? styles.btn : styles.btnGhost}
          onClick={() => setStatusFilter("closed")}
        >
          {t("admin:reports.closedTab")}
        </button>
      </div>
      {filtered.length === 0 && (
        <p>
          {t(
            `admin:reports.${statusFilter === "open" ? "emptyOpen" : "emptyClosed"}`,
          )}
        </p>
      )}
      {filtered.map((r) => (
        <section className={styles.card} key={r.id}>
          <div className={styles.row}>
            <div className={styles.grow}>
              <div className={styles.primaryText}>
                ⚠️{" "}
                {t(
                  `admin:reports.${r.subject_type === "post" ? "postSubject" : "actorSubject"}`,
                  { subject: r.subject },
                )}
              </div>
              <div className={styles.subText}>
                {t("admin:reports.summary", {
                  reporter: r.reporter,
                  reason: findReportReasonLabel(r.reason_type, t),
                  destination:
                    r.destination === "local"
                      ? t("admin:reports.localDestination")
                      : r.remote_host,
                })}
              </div>
              <div className={styles.subText}>
                {new Date(r.created_at).toLocaleString()} / {r.status}
              </div>
              {r.reason_text && <p>{r.reason_text}</p>}
              <div className={styles.authStatus}>
                <a href={`/@${r.subject}`} target="_blank" rel="noreferrer">
                  {t("admin:reports.viewActor")}
                </a>
                {r.subject_post_id && (
                  <a
                    href={`/notes/${r.subject_post_id}`}
                    target="_blank"
                    rel="noreferrer"
                  >
                    {t("admin:reports.viewPost")}
                  </a>
                )}
              </div>
            </div>
          </div>
          <div className={styles.authStatus}>
            {r.status === "open" && (
              <button
                className={styles.btnGhost}
                onClick={() => act(() => api.admin.closeReport(r.id))}
              >
                {t("admin:reports.closeButton")}
              </button>
            )}
            {r.destination === "remote" && !r.forwarded_at && (
              <button
                className={styles.btn}
                onClick={() => act(() => api.admin.forwardReport(r.id))}
              >
                {t("admin:reports.forwardButton")}
              </button>
            )}
            {r.subject_post_id && (
              <button
                className={styles.btnDanger}
                onClick={() => act(() => api.admin.deleteReportedPost(r.id))}
              >
                {t("admin:reports.deletePostButton")}
              </button>
            )}
            <button
              className={styles.btnDanger}
              onClick={() => act(() => api.admin.suspendReportedUser(r.id))}
            >
              {t("admin:reports.suspendUserButton")}
            </button>
            <button
              className={styles.btnGhost}
              onClick={() => loadComments(r.id)}
            >
              {t("admin:reports.commentsButton")}
            </button>
          </div>
          {comments[r.id] && (
            <div>
              {comments[r.id].map((c) => (
                <p key={c.id}>
                  <strong>{c.author}</strong>: {c.body}
                </p>
              ))}
              <textarea
                className={styles.input}
                value={drafts[r.id] ?? ""}
                maxLength={2000}
                onChange={(e) =>
                  setDrafts((v) => ({ ...v, [r.id]: e.target.value }))
                }
              />
              <button className={styles.btn} onClick={() => addComment(r.id)}>
                {t("admin:reports.addCommentButton")}
              </button>
            </div>
          )}
        </section>
      ))}
    </div>
  );
}
