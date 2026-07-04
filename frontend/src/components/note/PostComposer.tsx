import { ChangeEvent, FormEvent, KeyboardEvent, useEffect, useRef, useState } from "react";
import { api, DriveFile, Note, getErrorMessage } from "../../api/client";
import { acct, calcRemaining, displayName } from "../../lib/format";
import styles from "./PostComposer.module.css";

interface PostComposerProps {
  onPosted?: (note: Note) => void;
  autoFocus?: boolean;
  /** 指定時は返信として投稿する（配信先は元ポストのネットワークに自動ルーティング）。 */
  replyTo?: Note;
}

export default function PostComposer({ onPosted, autoFocus, replyTo }: PostComposerProps) {
  const [text, setText] = useState("");
  const [deliverFedi, setDeliverFedi] = useState(true);
  const [deliverBsky, setDeliverBsky] = useState(true);
  const [posting, setPosting] = useState(false);
  const [error, setError] = useState("");
  const [attached, setAttached] = useState<DriveFile | null>(null);
  const [uploading, setUploading] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (autoFocus) textareaRef.current?.focus();
  }, [autoFocus]);

  // 返信時は配信先が元ポストのネットワークに固定される。fedi リモートへの返信のみ
  // Fedi の緩い上限、それ以外（bsky / ローカル・seiran＝両方）は Bsky の厳しい上限を適用。
  const effectiveBsky = replyTo ? replyTo.user.actorType !== "fedi" : deliverBsky;
  const remaining = calcRemaining(text, effectiveBsky);
  const overLimit = remaining < 0;

  async function handlePost(e: FormEvent) {
    e.preventDefault();
    if (!text.trim() || overLimit || posting) return;
    setError("");
    setPosting(true);
    try {
      const attachmentIds = attached ? [attached.id] : [];
      const note = await api.notes.create(
        text.trim(),
        replyTo ? true : deliverFedi,
        replyTo ? true : deliverBsky,
        attachmentIds,
        replyTo?.id
      );
      setText("");
      setAttached(null);
      onPosted?.(note);
      textareaRef.current?.focus();
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setPosting(false);
    }
  }

  async function handleFileSelect(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";
    setError("");
    setUploading(true);
    try {
      setAttached(await api.media.upload(file));
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setUploading(false);
    }
  }

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      handlePost(e as unknown as FormEvent);
    }
  }

  return (
    <form onSubmit={handlePost} className={styles.form}>
      {replyTo && (
        <div className={styles.replyBanner}>
          <span className={styles.replyTo}>
            返信先: <strong>{displayName(replyTo)}</strong> {acct(replyTo)}
          </span>
          <span className={styles.replySnippet}>{replyTo.text}</span>
        </div>
      )}

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
        onKeyDown={handleKeyDown}
        className={styles.textarea}
        placeholder={replyTo ? "返信を入力" : "いまどうしてる？"}
        rows={3}
      />

      <div className={styles.scopeRow}>
        {!replyTo && (
          <>
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
          </>
        )}
        <button
          type="button"
          className={styles.attachBtn}
          onClick={() => fileInputRef.current?.click()}
          disabled={uploading || !!attached}
          title="画像を添付"
        >
          📎
        </button>
        {replyTo && (
          <span className={styles.replyScopeNote}>
            {replyTo.user.actorType === "fedi"
              ? "Fediverse に返信"
              : replyTo.user.actorType === "bsky"
              ? "Bluesky に返信"
              : "元ポストのネットワークに返信"}
          </span>
        )}
        {uploading && <span className={styles.spinner} />}
      </div>

      {attached && (
        <div className={styles.attachPreview}>
          <img src={attached.url} alt="添付画像" className={styles.attachThumb} />
          <button
            type="button"
            className={styles.attachRemoveBtn}
            onClick={() => setAttached(null)}
            title="添付を解除"
          >
            ×
          </button>
        </div>
      )}

      {effectiveBsky && overLimit && (
        <p className={styles.guide}>
          {replyTo
            ? "Bluesky の文字数制限を超えています。"
            : "Bluesky の文字数制限を超えています。Bsky をオフにすると投稿できます。"}
        </p>
      )}

      <div className={styles.footer}>
        <span className={`${styles.charCount} ${overLimit ? styles.charCountOver : ""}`}>
          残り {remaining}
        </span>
        {error && <span className={styles.error}>{error}</span>}
        <button type="submit" className={styles.postBtn} disabled={posting || !text.trim() || overLimit}>
          {posting ? "投稿中..." : "投稿"}
        </button>
      </div>
    </form>
  );
}
