import { ChangeEvent, FormEvent, KeyboardEvent, useEffect, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, Note, DriveFile, getErrorMessage } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import styles from "./Timeline.module.css";

type Tab = "local" | "home";

function countGraphemes(text: string): number {
  const seg = new Intl.Segmenter();
  return [...seg.segment(text)].length;
}

function countUtf8Bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

function calcRemaining(text: string, deliverBsky: boolean): number {
  const maxBytes = deliverBsky ? 3_000 : 10_000;
  const maxGraphemes = deliverBsky ? 300 : 3_000;
  const graphemes = countGraphemes(text);
  const bytes = countUtf8Bytes(text);
  return Math.min(maxGraphemes - graphemes, Math.floor((maxBytes - bytes) / 3));
}

export default function Timeline() {
  const { user, logout } = useAuth();
  const navigate = useNavigate();
  const [tab, setTab] = useState<Tab>("local");
  const [notes, setNotes] = useState<Note[]>([]);
  const [text, setText] = useState("");
  const [deliverFedi, setDeliverFedi] = useState(true);
  const [deliverBsky, setDeliverBsky] = useState(true);
  const [posting, setPosting] = useState(false);
  const [postError, setPostError] = useState("");
  const [loading, setLoading] = useState(true);
  const [attached, setAttached] = useState<DriveFile | null>(null);
  const [uploading, setUploading] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setLoading(true);
    const fetch = tab === "home"
      ? api.notes.homeTimeline({ limit: 30 })
      : api.notes.localTimeline({ limit: 30 });
    fetch.then(setNotes).finally(() => setLoading(false));
  }, [tab]);

  const remaining = calcRemaining(text, deliverBsky);
  const overLimit = remaining < 0;

  async function handlePost(e: FormEvent) {
    e.preventDefault();
    if (!text.trim() || overLimit) return;
    setPostError("");
    setPosting(true);
    try {
      const attachmentIds = attached ? [attached.id] : [];
      const note = await api.notes.create(text.trim(), deliverFedi, deliverBsky, attachmentIds);
      setNotes((prev) => [note, ...prev]);
      setText("");
      setAttached(null);
      textareaRef.current?.focus();
    } catch (err) {
      setPostError(getErrorMessage(err));
    } finally {
      setPosting(false);
    }
  }

  async function handleFileSelect(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";
    setPostError("");
    setUploading(true);
    try {
      const driveFile = await api.media.upload(file);
      setAttached(driveFile);
    } catch (err) {
      setPostError(getErrorMessage(err));
    } finally {
      setUploading(false);
    }
  }

  function handleRemoveAttachment() {
    setAttached(null);
  }

  function handleTextareaKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      if (text.trim() && !overLimit && !posting) {
        handlePost(e as unknown as FormEvent);
      }
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

  function handleUserClick(note: Note) {
    const q = note.user.domain && note.user.domain !== window.location.hostname
      ? `${note.user.username}@${note.user.domain}`
      : note.user.username;
    navigate(`/profile?q=${encodeURIComponent(q)}`);
  }

  function displayName(note: Note) {
    return note.user.display_name || note.user.username;
  }

  function acct(note: Note) {
    return note.user.domain
      ? `@${note.user.username}@${note.user.domain}`
      : `@${note.user.username}`;
  }

  return (
    <div className={styles.layout}>
      <header className={styles.header}>
        <span className={styles.logo}>seiran</span>
        <div className={styles.headerRight}>
          <span
            className={styles.username}
            style={{ cursor: "pointer" }}
            onClick={() => navigate(`/profile?q=${user?.username}`)}
          >
            @{user?.username}
          </span>
          <button className={styles.logoutBtn} onClick={handleLogout}>
            ログアウト
          </button>
        </div>
      </header>

      <main className={styles.main}>
        <form onSubmit={handlePost} className={styles.postForm}>
          <input
            ref={fileInputRef}
            type="file"
            accept="image/*"
            style={{ display: "none" }}
            onChange={handleFileSelect}
          />
          <textarea
            ref={textareaRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={handleTextareaKeyDown}
            className={styles.textarea}
            placeholder="いまどうしてる？"
            rows={3}
          />

          <div className={styles.scopeRow}>
            <button
              type="button"
              className={`${styles.scopeBtn} ${deliverFedi ? styles.scopeActive : ""}`}
              onClick={() => setDeliverFedi((v) => !v)}
            >
              Fedi 🌐
            </button>
            <button
              type="button"
              className={`${styles.scopeBtn} ${deliverBsky ? styles.scopeActive : ""}`}
              onClick={() => setDeliverBsky((v) => !v)}
            >
              Bsky 🦋
            </button>
            <button
              type="button"
              className={styles.attachBtn}
              onClick={() => fileInputRef.current?.click()}
              disabled={uploading || !!attached}
              title="画像を添付"
            >
              📎
            </button>
            {uploading && <span className={styles.spinner} />}
          </div>

          {attached && (
            <div className={styles.attachPreview}>
              <img src={attached.url} alt="添付画像" className={styles.attachThumb} />
              <button
                type="button"
                className={styles.attachRemoveBtn}
                onClick={handleRemoveAttachment}
                title="添付を解除"
              >
                ×
              </button>
            </div>
          )}

          {deliverBsky && overLimit && (
            <p className={styles.scopeGuide}>
              Bluesky の文字数制限を超えています。Bsky をオフにすると投稿できます。
            </p>
          )}

          <div className={styles.postFooter}>
            <span className={`${styles.charCount} ${overLimit ? styles.charCountOver : ""}`}>
              残り {remaining}
            </span>
            {postError && <span className={styles.postError}>{postError}</span>}
            <button
              type="submit"
              className={styles.postBtn}
              disabled={posting || !text.trim() || overLimit}
            >
              {posting ? "投稿中..." : "投稿"}
            </button>
          </div>
        </form>

        <div className={styles.tabs}>
          <button
            className={`${styles.tab} ${tab === "local" ? styles.activeTab : ""}`}
            onClick={() => setTab("local")}
          >
            ローカル
          </button>
          <button
            className={`${styles.tab} ${tab === "home" ? styles.activeTab : ""}`}
            onClick={() => setTab("home")}
          >
            ホーム
          </button>
        </div>

        <div className={styles.timeline}>
          {loading && <p className={styles.message}>読み込み中...</p>}
          {!loading && notes.length === 0 && (
            <p className={styles.message}>
              {tab === "home"
                ? "フォロー中のユーザーの投稿がここに表示されます。"
                : "まだ投稿がありません。最初の投稿をしてみましょう！"}
            </p>
          )}
          {notes.map((note) => (
            <article key={note.id} className={styles.note}>
              <div className={styles.noteHeader}>
                <button
                  className={styles.noteUserBtn}
                  onClick={() => handleUserClick(note)}
                >
                  <span className={styles.noteDisplayName}>{displayName(note)}</span>
                  <span className={styles.noteAcct}>{acct(note)}</span>
                </button>
                <Link to={`/notes/${note.id}`} className={styles.noteTime}>
                  <time>{formatDate(note.created_at)}</time>
                </Link>
              </div>
              <p className={styles.noteBody}>{note.text}</p>
            </article>
          ))}
        </div>
      </main>
    </div>
  );
}
