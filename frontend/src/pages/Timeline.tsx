import { FormEvent, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { api, Note } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import styles from "./Timeline.module.css";

export default function Timeline() {
  const { user, logout } = useAuth();
  const navigate = useNavigate();
  const [notes, setNotes] = useState<Note[]>([]);
  const [text, setText] = useState("");
  const [posting, setPosting] = useState(false);
  const [postError, setPostError] = useState("");
  const [loading, setLoading] = useState(true);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    api.notes
      .localTimeline({ limit: 30 })
      .then(setNotes)
      .finally(() => setLoading(false));
  }, []);

  async function handlePost(e: FormEvent) {
    e.preventDefault();
    if (!text.trim()) return;
    setPostError("");
    setPosting(true);
    try {
      const note = await api.notes.create(text.trim());
      setNotes((prev) => [note, ...prev]);
      setText("");
      textareaRef.current?.focus();
    } catch (err) {
      setPostError(err instanceof Error ? err.message : "投稿に失敗しました");
    } finally {
      setPosting(false);
    }
  }

  function handleLogout() {
    logout();
    navigate("/login");
  }

  function formatDate(iso: string) {
    return new Date(iso).toLocaleString("ja-JP", {
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  return (
    <div className={styles.layout}>
      <header className={styles.header}>
        <span className={styles.logo}>seiran</span>
        <div className={styles.headerRight}>
          <span className={styles.username}>@{user?.username}</span>
          <button className={styles.logoutBtn} onClick={handleLogout}>
            ログアウト
          </button>
        </div>
      </header>

      <main className={styles.main}>
        <form onSubmit={handlePost} className={styles.postForm}>
          <textarea
            ref={textareaRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            className={styles.textarea}
            placeholder="いまどうしてる？"
            rows={3}
            maxLength={500}
          />
          <div className={styles.postFooter}>
            <span className={styles.charCount}>{text.length} / 500</span>
            {postError && <span className={styles.postError}>{postError}</span>}
            <button
              type="submit"
              className={styles.postBtn}
              disabled={posting || !text.trim()}
            >
              {posting ? "投稿中..." : "投稿"}
            </button>
          </div>
        </form>

        <div className={styles.timeline}>
          {loading && <p className={styles.message}>読み込み中...</p>}
          {!loading && notes.length === 0 && (
            <p className={styles.message}>まだ投稿がありません。最初の投稿をしてみましょう！</p>
          )}
          {notes.map((note) => (
            <article key={note.id} className={styles.note}>
              <div className={styles.noteHeader}>
                <span className={styles.noteUser}>@{note.user.username}</span>
                <time className={styles.noteTime}>{formatDate(note.created_at)}</time>
              </div>
              <p className={styles.noteBody}>{note.text}</p>
            </article>
          ))}
        </div>
      </main>
    </div>
  );
}
