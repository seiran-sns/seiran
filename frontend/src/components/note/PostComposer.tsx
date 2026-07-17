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

type Visibility = "public" | "unlisted" | "followers_only";

/** 返信先ポストの可視性から、この返信の選択肢・デフォルト・強制値を算出する。 */
function replyVisibilityConstraint(replyTo?: Note): {
  forced: Visibility | null;
  defaultValue: Visibility;
} {
  const parent = replyTo?.visibility;
  if (parent === "followers_only") {
    return { forced: "followers_only", defaultValue: "followers_only" };
  }
  if (parent === "unlisted") {
    return { forced: null, defaultValue: "unlisted" };
  }
  // undefined(public) / "direct" / 想定外値 → 制約なし
  return { forced: null, defaultValue: "public" };
}

export default function PostComposer({ onPosted, autoFocus, replyTo }: PostComposerProps) {
  const [text, setText] = useState("");
  const [deliverFedi, setDeliverFedi] = useState(true);
  const [deliverBsky, setDeliverBsky] = useState(true);
  const replyConstraint = replyTo ? replyVisibilityConstraint(replyTo) : null;
  const [visibility, setVisibility] = useState<Visibility>(() => replyConstraint?.defaultValue ?? "public");
  const [guideMessage, setGuideMessage] = useState<string | null>(null);
  const [posting, setPosting] = useState(false);
  const [error, setError] = useState("");
  const [attached, setAttached] = useState<DriveFile | null>(null);
  const [uploading, setUploading] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const guideTimerRef = useRef<number | null>(null);

  useEffect(() => {
    if (autoFocus) textareaRef.current?.focus();
  }, [autoFocus]);

  useEffect(() => {
    return () => {
      if (guideTimerRef.current) window.clearTimeout(guideTimerRef.current);
    };
  }, []);

  function showGuide(message: string) {
    setGuideMessage(message);
    if (guideTimerRef.current) window.clearTimeout(guideTimerRef.current);
    guideTimerRef.current = window.setTimeout(() => setGuideMessage(null), 3200);
  }

  // Bsky はプロトコル上 followers_only（プライベート）投稿を配信できないため相互排他。
  // unlisted（ひかえめ）は Bsky 配送と両立できる。
  function handleVisibilityChange(next: Visibility) {
    if (next === "followers_only" && deliverBsky) {
      showGuide("Bsky配送がオンの間はプライベートを選べません。Bsky配送をオフにすると変更できます。");
      return;
    }
    setVisibility(next);
  }

  function handleToggleBsky() {
    if (!deliverBsky && visibility === "followers_only") {
      showGuide("Bsky配送は「プライベート」以外の投稿で対応しています。可視性を変更すると配送できます。");
      return;
    }
    setDeliverBsky((v) => !v);
  }

  // 返信時は配信先が元ポストのネットワークに固定される。fedi リモートへの返信のみ
  // Fedi の緩い上限、それ以外（bsky / ローカル・seiran＝両方）は Bsky の厳しい上限を適用。
  // 可視性がプライベートの場合は Bsky に配送されないため上限判定からも除外する。
  const effectiveBsky = replyTo
    ? replyTo.user.actorType !== "fedi" && visibility !== "followers_only"
    : deliverBsky;
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
        replyTo?.id,
        undefined,
        visibility
      );
      setText("");
      setAttached(null);
      setVisibility(replyConstraint?.defaultValue ?? "public");
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
        accept="image/*,video/*,audio/*"
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
              onClick={handleToggleBsky}
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
          title="画像・動画・音声を添付"
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

      {replyConstraint?.forced ? (
        <div className={styles.visibilityRow}>
          <span className={styles.replyScopeNote}>
            🔒️ 非公開の投稿への返信のため、この返信も非公開になります
          </span>
        </div>
      ) : (
        <div className={styles.visibilityRow}>
          <button
            type="button"
            className={`${styles.scopeBtn} ${visibility === "public" ? styles.scopeActive : ""}`}
            onClick={() => handleVisibilityChange("public")}
          >
            👥 パブリック
          </button>
          <button
            type="button"
            className={`${styles.scopeBtn} ${visibility === "unlisted" ? styles.scopeActive : ""}`}
            onClick={() => handleVisibilityChange("unlisted")}
          >
            🤫 ひかえめ
          </button>
          <button
            type="button"
            className={`${styles.scopeBtn} ${visibility === "followers_only" ? styles.scopeActive : ""}`}
            onClick={() => handleVisibilityChange("followers_only")}
          >
            🔒️ プライベート
          </button>
          {guideMessage && (
            <span className={styles.popover} role="status">
              {guideMessage}
            </span>
          )}
        </div>
      )}

      {attached && (
        <div className={styles.attachPreview}>
          {attached.mimeType.startsWith("video/") ? (
            <video src={attached.url} controls className={styles.attachThumb} />
          ) : attached.mimeType.startsWith("audio/") ? (
            <audio src={attached.url} controls className={styles.attachAudio} />
          ) : (
            <img src={attached.url} alt="添付画像" className={styles.attachThumb} />
          )}
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
