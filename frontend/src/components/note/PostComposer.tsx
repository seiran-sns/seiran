import { ChangeEvent, FormEvent, KeyboardEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, DriveFile, Note, getErrorMessage } from "../../api/client";
import { acct, calcRemaining, displayName } from "../../lib/format";
import styles from "./PostComposer.module.css";

interface PostComposerProps {
  onPosted?: (note: Note) => void;
  autoFocus?: boolean;
  /** 指定時は返信として投稿する（配信先は元ポストのネットワークに自動ルーティング）。 */
  replyTo?: Note;
  /** 本文の初期値（ハッシュタグ入力済みでの投稿ダイアログ起動等）。 */
  initialText?: string;
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

export default function PostComposer({ onPosted, autoFocus, replyTo, initialText }: PostComposerProps) {
  const { t } = useTranslation();
  const [text, setText] = useState(initialText ?? "");
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
    if (!autoFocus) return;
    const el = textareaRef.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
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
      showGuide(t("home:postComposer.bskyPrivateConflict"));
      return;
    }
    setVisibility(next);
  }

  function handleToggleBsky() {
    if (!deliverBsky && visibility === "followers_only") {
      showGuide(t("home:postComposer.bskyVisibilityConflict"));
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
            {t("home:postComposer.replyToPrefix")} <strong>{displayName(replyTo)}</strong> {acct(replyTo)}
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
        placeholder={replyTo ? t("home:postComposer.replyPlaceholder") : t("home:postComposer.placeholder")}
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
          title={t("home:postComposer.attachTitle")}
        >
          📎
        </button>
        {replyTo && (
          <span className={styles.replyScopeNote}>
            {replyTo.user.actorType === "fedi"
              ? t("home:postComposer.replyToFedi")
              : replyTo.user.actorType === "bsky"
              ? t("home:postComposer.replyToBsky")
              : t("home:postComposer.replyToSameNetwork")}
          </span>
        )}
        {uploading && <span className={styles.spinner} />}
      </div>

      {replyConstraint?.forced ? (
        <div className={styles.visibilityRow}>
          <span className={styles.replyScopeNote}>
            🔒️ {t("home:postComposer.forcedPrivateNote")}
          </span>
        </div>
      ) : (
        <div className={styles.visibilityRow}>
          <button
            type="button"
            className={`${styles.scopeBtn} ${visibility === "public" ? styles.scopeActive : ""}`}
            onClick={() => handleVisibilityChange("public")}
          >
            👥 {t("home:postComposer.visibilityPublic")}
          </button>
          <button
            type="button"
            className={`${styles.scopeBtn} ${visibility === "unlisted" ? styles.scopeActive : ""}`}
            onClick={() => handleVisibilityChange("unlisted")}
          >
            🤫 {t("home:postComposer.visibilityUnlisted")}
          </button>
          <button
            type="button"
            className={`${styles.scopeBtn} ${visibility === "followers_only" ? styles.scopeActive : ""}`}
            onClick={() => handleVisibilityChange("followers_only")}
          >
            🔒️ {t("home:postComposer.visibilityPrivate")}
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
            <img src={attached.url} alt={t("home:postComposer.attachmentAlt")} className={styles.attachThumb} />
          )}
          <button
            type="button"
            className={styles.attachRemoveBtn}
            onClick={() => setAttached(null)}
            title={t("home:postComposer.removeAttachmentTitle")}
          >
            ×
          </button>
        </div>
      )}

      {effectiveBsky && overLimit && (
        <p className={styles.guide}>
          {replyTo
            ? t("home:postComposer.overLimitReply")
            : t("home:postComposer.overLimitDefault")}
        </p>
      )}

      <div className={styles.footer}>
        <span className={`${styles.charCount} ${overLimit ? styles.charCountOver : ""}`}>
          {t("home:postComposer.remainingCount", { count: remaining })}
        </span>
        {error && <span className={styles.error}>{error}</span>}
        <button type="submit" className={styles.postBtn} disabled={posting || !text.trim() || overLimit}>
          {posting ? t("home:postComposer.posting") : t("home:postComposer.postButton")}
        </button>
      </div>
    </form>
  );
}
