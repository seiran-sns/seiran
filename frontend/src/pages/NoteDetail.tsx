import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { api, Note, getErrorMessage } from "../api/client";
import styles from "./UserProfile.module.css";

export default function NoteDetail() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [note, setNote] = useState<Note | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!id) return;
    setLoading(true);
    api.notes
      .get(id)
      .then((n) => setNote(n))
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }, [id]);

  function formatDate(iso: string) {
    return new Date(iso).toLocaleString("ja-JP", {
      year: "numeric",
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  const acct = note
    ? note.user.domain
      ? `@${note.user.username}@${note.user.domain}`
      : `@${note.user.username}`
    : "";

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <button className={styles.backBtn} onClick={() => navigate(-1)}>
          ← 戻る
        </button>
        <span className={styles.logo}>seiran</span>
      </header>

      <main className={styles.main}>
        {loading && <p className={styles.message}>読み込み中...</p>}
        {error && <p className={styles.error}>{error}</p>}

        {note && (
          <article className={styles.post} style={{ padding: "1.25rem" }}>
            <div className={styles.profileNames} style={{ marginBottom: "0.75rem" }}>
              <span className={styles.displayName}>
                {note.user.display_name || note.user.username}
              </span>
              <span className={styles.acct}>{acct}</span>
            </div>
            <p className={styles.postBody} style={{ fontSize: "1.1rem", lineHeight: 1.6 }}>
              {note.text}
            </p>
            <time className={styles.postTime}>{formatDate(note.created_at)}</time>
          </article>
        )}
      </main>
    </div>
  );
}
