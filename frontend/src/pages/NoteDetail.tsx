import { useEffect, useState } from "react";
import { useNavigate, useParams, Link } from "react-router-dom";
import { api, Note, getErrorMessage } from "../api/client";
import styles from "./UserProfile.module.css";

export default function NoteDetail() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [note, setNote] = useState<Note | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [before, setBefore] = useState<Note[]>([]);
  const [after, setAfter] = useState<Note[]>([]);
  const [contextLoading, setContextLoading] = useState(false);

  useEffect(() => {
    if (!id) return;
    setLoading(true);
    setBefore([]);
    setAfter([]);
    api.notes
      .get(id)
      .then((n) => {
        setNote(n);
        // コンテキストを並列で取得
        setContextLoading(true);
        api.notes
          .context(id)
          .then((ctx) => {
            setBefore(ctx.before);
            setAfter(ctx.after);
          })
          .catch(() => {}) // context 取得失敗は無視
          .finally(() => setContextLoading(false));
      })
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

  function noteAcct(n: Note) {
    return n.user.domain
      ? `@${n.user.username}@${n.user.domain}`
      : `@${n.user.username}`;
  }

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

        {/* 前のノート（降順で来るのでreverse して古い順に表示） */}
        {before.length > 0 && (
          <section>
            <p className={styles.message} style={{ fontSize: "0.85rem", color: "#666" }}>
              それより前のノート
            </p>
            {[...before].reverse().map((n) => (
              <Link
                key={n.id}
                to={`/notes/${n.id}`}
                style={{ textDecoration: "none", color: "inherit" }}
              >
                <article
                  className={styles.post}
                  style={{ opacity: 0.7, padding: "0.75rem 1.25rem" }}
                >
                  <span className={styles.acct}>{noteAcct(n)}</span>
                  <p className={styles.postBody}>{n.text}</p>
                  <time className={styles.postTime}>{formatDate(n.created_at)}</time>
                </article>
              </Link>
            ))}
          </section>
        )}

        {/* メインノート */}
        {note && (
          <article
            className={styles.post}
            style={{
              padding: "1.25rem",
              borderLeft: "3px solid var(--accent, #6b7cff)",
            }}
          >
            <div className={styles.profileNames} style={{ marginBottom: "0.75rem" }}>
              <span className={styles.displayName}>
                {note.user.display_name || note.user.username}
              </span>
              <span className={styles.acct}>{noteAcct(note)}</span>
            </div>
            <p
              className={styles.postBody}
              style={{ fontSize: "1.1rem", lineHeight: 1.6 }}
            >
              {note.text}
            </p>
            <time className={styles.postTime}>{formatDate(note.created_at)}</time>
          </article>
        )}

        {/* 後のノート */}
        {after.length > 0 && (
          <section>
            <p className={styles.message} style={{ fontSize: "0.85rem", color: "#666" }}>
              それより後のノート
            </p>
            {after.map((n) => (
              <Link
                key={n.id}
                to={`/notes/${n.id}`}
                style={{ textDecoration: "none", color: "inherit" }}
              >
                <article
                  className={styles.post}
                  style={{ opacity: 0.7, padding: "0.75rem 1.25rem" }}
                >
                  <span className={styles.acct}>{noteAcct(n)}</span>
                  <p className={styles.postBody}>{n.text}</p>
                  <time className={styles.postTime}>{formatDate(n.created_at)}</time>
                </article>
              </Link>
            ))}
          </section>
        )}

        {contextLoading && (
          <p className={styles.message} style={{ fontSize: "0.85rem" }}>
            周辺ノートを読み込み中...
          </p>
        )}
      </main>
    </div>
  );
}
