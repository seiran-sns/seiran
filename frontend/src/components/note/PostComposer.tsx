import { ChangeEvent, FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, DriveFile, Note, getErrorMessage } from "../../api/client";
import { acct, calcRemaining, displayName } from "../../lib/format";
import { useAuth } from "../../contexts/AuthContext";
import {
  clearComposerDraft,
  DraftTarget,
  loadComposerDraft,
  onComposerDraftRefresh,
  saveComposerDraft,
} from "../../lib/composerDraft";
import styles from "./PostComposer.module.css";
import ComposerEditor from "./ComposerEditor";
import TwemojiEmoji from "../common/TwemojiEmoji";
import blueskyLogo from "../../assets/bluesky-logo.svg";
import fediverseLogo from "../../assets/fediverse-logo.svg";

interface PostComposerProps {
  onPosted?: (note: Note) => void;
  autoFocus?: boolean;
  /** 指定時は返信として投稿する（配信先は元ポストのネットワークに自動ルーティング）。 */
  replyTo?: Note;
  /** 指定時は対象ポストを引用して投稿する。 */
  quoteTo?: Note;
  /** 本文の初期値（ハッシュタグ入力済みでの投稿ダイアログ起動等）。 */
  initialText?: string;
}

type Visibility = "public" | "unlisted" | "followers_only";

/** 公開範囲を狭い順に並べたもの（`direct`は別軸のためここには含めない）。 */
const VISIBILITY_NARROWING_ORDER: Visibility[] = ["public", "unlisted", "followers_only"];

/**
 * 返信先ポストの可視性から、この返信で選択可能な公開範囲（狭める方向のみ）・デフォルト値を
 * 算出する。バックエンド（`ReplyContext::resolve_visibility`）は親より広い範囲への変更も
 * 技術的には許容するが、UIでは「狭めることはできるべき」という要件に合わせて狭める方向の
 * 選択肢のみ提示する。
 */
export function replyVisibilityConstraint(replyTo?: Note): {
  options: Visibility[];
  defaultValue: Visibility;
} {
  const parent = replyTo?.visibility;
  if (parent === "followers_only") {
    return { options: ["followers_only"], defaultValue: "followers_only" };
  }
  if (parent === "unlisted") {
    return { options: ["unlisted", "followers_only"], defaultValue: "unlisted" };
  }
  // undefined(public) / "direct" / 想定外値 → 制約なし（3段階すべて選択可）
  return { options: VISIBILITY_NARROWING_ORDER, defaultValue: "public" };
}

/** 引用対象ポストの可視性から、この引用のデフォルト可視性を算出する。 */
export function quoteVisibilityConstraint(quoteTo?: Note): {
  defaultValue: Visibility;
} {
  const parent = quoteTo?.visibility;
  if (parent === "unlisted") {
    return { defaultValue: "unlisted" };
  }
  return { defaultValue: "public" };
}

export default function PostComposer({
  onPosted,
  autoFocus,
  replyTo,
  quoteTo,
  initialText,
}: PostComposerProps) {
  const { t } = useTranslation();
  const { user } = useAuth();
  const replyConstraint = replyTo ? replyVisibilityConstraint(replyTo) : null;
  const quoteConstraint = quoteTo ? quoteVisibilityConstraint(quoteTo) : null;
  // 返信の配送先トグルは、返信先ポストが実際に持つプロトコル実体のみ表示する（持たない
  // プロトコルへ配送すると親と無関係な独立ポストとして誤配信されるため）。Bsky は
  // followers_only 可視性を配信できないため、親自体が非公開（＝この返信も非公開固定）
  // の場合は通常投稿時のプライベートボタンと同様に選択肢から外す。
  const fediReplyAllowed = !replyTo || replyTo.replyFediAllowed;
  const bskyReplyAllowed =
    !replyTo || (replyTo.replyBskyAllowed && replyTo.visibility !== "followers_only");

  // 投稿ダイアログを閉じても書きかけを失わないよう、ユーザー×対象ポスト単位でローカル
  // ストレージに自動保存する（#193）。マウント時に一度だけ読み込み、以降は入力の都度保存。
  const draftTarget: DraftTarget | null = useMemo(() => {
    if (!user) return null;
    if (replyTo) return { mode: "reply", userId: user.id, postId: replyTo.id };
    if (quoteTo) return { mode: "quote", userId: user.id, postId: quoteTo.id };
    return { mode: "compose", userId: user.id };
  }, [user, replyTo, quoteTo]);
  const [initialDraft] = useState(() =>
    draftTarget ? loadComposerDraft(draftTarget) : null,
  );

  const [text, setText] = useState(initialDraft?.text ?? initialText ?? "");
  const [deliverFedi, setDeliverFedi] = useState(initialDraft?.deliverFedi ?? fediReplyAllowed);
  const [deliverBsky, setDeliverBsky] = useState(initialDraft?.deliverBsky ?? bskyReplyAllowed);
  const [visibility, setVisibility] = useState<Visibility>(() => {
    if (initialDraft && !replyTo) return initialDraft.visibility;
    return (
      replyConstraint?.defaultValue ??
      quoteConstraint?.defaultValue ??
      "public"
    );
  });
  const [posting, setPosting] = useState(false);
  const [error, setError] = useState("");
  const [attached, setAttached] = useState<DriveFile | null>(initialDraft?.attached ?? null);
  const [uploading, setUploading] = useState(false);
  const [showPrivateTooltip, setShowPrivateTooltip] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const privateTooltipTimerRef = useRef<number | null>(null);

  useEffect(() => {
    if (!autoFocus) return;
    // ComposerEditor が装飾DOMの構築後にフォーカスとcaret復元を行う。
  }, [autoFocus]);

  useEffect(() => {
    if (!draftTarget) return;
    saveComposerDraft(draftTarget, { text, attached, deliverFedi, deliverBsky, visibility });
  }, [draftTarget, text, attached, deliverFedi, deliverBsky, visibility]);

  useEffect(() => {
    if (!draftTarget) return;
    return onComposerDraftRefresh((target) => {
      if (JSON.stringify(target) !== JSON.stringify(draftTarget)) return;
      const draft = loadComposerDraft(draftTarget);
      setText(draft?.text ?? "");
      setAttached(draft?.attached ?? null);
      setDeliverFedi(draft?.deliverFedi ?? fediReplyAllowed);
      setDeliverBsky(draft?.deliverBsky ?? bskyReplyAllowed);
      setVisibility(draft?.visibility ?? "public");
    });
  }, [draftTarget, fediReplyAllowed, bskyReplyAllowed]);

  useEffect(() => {
    return () => {
      if (privateTooltipTimerRef.current) window.clearTimeout(privateTooltipTimerRef.current);
    };
  }, []);

  // Bsky 配送オンの間は🔒️プライベート投稿ボタンをグレーアウトする（Bsky はプロトコル上
  // followers_only を配信できないため相互排他）。吹き出しはホバー・クリックの両方で出す。
  function handlePrivateTooltipEnter() {
    if (!deliverBsky) return;
    setShowPrivateTooltip(true);
  }

  function handlePrivateTooltipLeave() {
    setShowPrivateTooltip(false);
  }

  function handlePrivateTooltipClick() {
    if (!deliverBsky) return;
    setShowPrivateTooltip(true);
    if (privateTooltipTimerRef.current) window.clearTimeout(privateTooltipTimerRef.current);
    privateTooltipTimerRef.current = window.setTimeout(
      () => setShowPrivateTooltip(false),
      3200,
    );
  }

  const remaining = calcRemaining(text, deliverBsky);
  const overLimit = remaining < 0;

  // 返信で表示する公開範囲ボタン。親から狭める方向のみ（replyVisibilityConstraint）、
  // かつ Bsky 配送中は followers_only を除く（プロトコル上フォロワー限定配信ができない
  // ため。新規投稿・引用は常に3段階全て表示し、followers_only はグレーアウト＋ツール
  // チップで理由を説明する既存方式のまま。返信は選択肢が親から動的に絞られるため、
  // 常にグレーアウトされ続けるボタンを出すより非表示にする方が分かりやすい）。
  const replyVisibilityOptions: Visibility[] = (
    replyConstraint?.options ?? []
  ).filter((v) => v !== "followers_only" || !deliverBsky);

  async function submitWithVisibility(v: Visibility) {
    if (!text.trim() || overLimit || posting) return;
    setError("");
    setPosting(true);
    try {
      const attachmentIds = attached ? [attached.id] : [];
      const note = await api.notes.create(
        text.trim(),
        deliverFedi,
        deliverBsky,
        attachmentIds,
        replyTo?.id,
        undefined,
        v,
        undefined,
        quoteTo?.id,
      );
      setText("");
      setAttached(null);
      setVisibility(replyConstraint?.defaultValue ?? "public");
      if (draftTarget) clearComposerDraft(draftTarget);
      onPosted?.(note);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setPosting(false);
    }
  }

  // キーボードショートカット（Ctrl+Enter等）送信時はデフォルトの公開範囲を使う
  // （通常投稿・返信いずれもボタン群からのクリックが主経路のため）。
  function handlePost(e: FormEvent) {
    e.preventDefault();
    submitWithVisibility(visibility);
  }

  async function uploadFile(file: File) {
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

  function handleFileSelect(e: ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    e.target.value = "";
    uploadFile(file);
  }

  return (
    <form
      onSubmit={handlePost}
      className={`${styles.form} ${replyTo ? styles.replyForm : ""}`}
    >
      {replyTo && (
        <div className={styles.replyBanner}>
          <span className={styles.replyTo}>
            {t("home:postComposer.replyToPrefix")}{" "}
            <strong>{displayName(replyTo)}</strong> {acct(replyTo)}
          </span>
          <span className={styles.replySnippet}>{replyTo.text}</span>
        </div>
      )}
      {quoteTo && (
        <div className={styles.replyBanner}>
          <span className={styles.replyTo}>
            {t("home:postComposer.quoteToPrefix")}{" "}
            <strong>{displayName(quoteTo)}</strong> {acct(quoteTo)}
          </span>
          <span className={styles.replySnippet}>{quoteTo.text}</span>
        </div>
      )}

      <input
        ref={fileInputRef}
        type="file"
        accept="image/*,video/*,audio/*"
        style={{ display: "none" }}
        onChange={handleFileSelect}
      />
      <ComposerEditor
        value={text}
        onChange={setText}
        onSubmitShortcut={() =>
          handlePost({ preventDefault() {} } as FormEvent)
        }
        onImagePaste={(file) => {
          if (!uploading && !attached) uploadFile(file);
        }}
        placeholder={
          replyTo
            ? t("home:postComposer.replyPlaceholder")
            : quoteTo
              ? t("home:postComposer.quotePlaceholder")
              : t("home:postComposer.placeholder")
        }
        autoFocus={autoFocus}
      />

      <div className={styles.scopeRow}>
        {uploading && <span className={styles.spinner} />}
      </div>

      {replyTo?.visibility === "followers_only" && (
        <div className={styles.visibilityRow}>
          <span className={styles.replyScopeNote}>
            <TwemojiEmoji emoji="🔒️" /> {t("home:postComposer.forcedPrivateNote")}
          </span>
        </div>
      )}

      <div className={styles.controlRow}>
        {fediReplyAllowed && (
          <button
            type="button"
            className={`${styles.iconBtn} ${deliverFedi ? styles.scopeActive : ""}`}
            onClick={() => setDeliverFedi((v) => !v)}
            title={t("home:postComposer.deliverFediHint")}
            aria-label={t("home:postComposer.deliverFediHint")}
          >
            <img className={styles.fediverseIcon} src={fediverseLogo} alt="" />
          </button>
        )}
        {bskyReplyAllowed && (
          <button
            type="button"
            className={`${styles.iconBtn} ${deliverBsky ? styles.scopeActive : ""}`}
            onClick={() => setDeliverBsky((v) => !v)}
            title={t("home:postComposer.deliverBskyHint")}
            aria-label={t("home:postComposer.deliverBskyHint")}
          >
            <img className={styles.blueskyIcon} src={blueskyLogo} alt="" />
          </button>
        )}
        <button
          type="button"
          className={styles.iconBtn}
          onClick={() => fileInputRef.current?.click()}
          disabled={uploading || !!attached}
          title={t("home:postComposer.attachTitle")}
          aria-label={t("home:postComposer.attachTitle")}
        >
          {uploading ? (
            <span className={styles.spinner} />
          ) : (
            <svg className={styles.pictureIcon} viewBox="0 0 24 24" aria-hidden="true">
              <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
              <circle cx="8.5" cy="9.5" r="1.6" />
              <path d="M4.5 18 9 12.8l3 3.2 3.5-4.2L19.5 18Z" />
            </svg>
          )}
        </button>
        <span
          className={`${styles.charCount} ${overLimit ? styles.charCountOver : ""}`}
        >
          {t("home:postComposer.remainingCount", { count: remaining })}
        </span>
      </div>

      <div className={styles.bottomRow}>
        {attached && (
          <div className={styles.attachPreview}>
            {attached.mimeType.startsWith("video/") ? (
              <video
                src={attached.url}
                controls
                className={styles.attachThumb}
              />
            ) : attached.mimeType.startsWith("audio/") ? (
              <audio
                src={attached.url}
                controls
                className={styles.attachAudio}
              />
            ) : (
              <img
                src={attached.url}
                alt={t("home:postComposer.attachmentAlt")}
                className={styles.attachThumb}
              />
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

        {deliverBsky && overLimit && (
          <p className={styles.guide}>
            {replyTo
              ? t("home:postComposer.overLimitReply")
              : t("home:postComposer.overLimitDefault")}
          </p>
        )}

        <div className={styles.footer}>
          {error && <span className={styles.error}>{error}</span>}
          {replyTo ? (
            <div className={styles.postBtnGroup}>
              {replyVisibilityOptions.includes("public") && (
                <button
                  type="button"
                  className={styles.postBtnVariant}
                  disabled={posting || !text.trim() || overLimit}
                  onClick={() => submitWithVisibility("public")}
                >
                  <span aria-hidden="true">
                    <TwemojiEmoji emoji="🌐" />
                  </span>
                  {t("home:postComposer.postButtonPublic")}
                </button>
              )}
              {replyVisibilityOptions.includes("unlisted") && (
                <button
                  type="button"
                  className={styles.postBtnVariant}
                  disabled={posting || !text.trim() || overLimit}
                  onClick={() => submitWithVisibility("unlisted")}
                >
                  <span aria-hidden="true">
                    <TwemojiEmoji emoji="🌙" />
                  </span>
                  {t("home:postComposer.postButtonUnlisted")}
                </button>
              )}
              {replyVisibilityOptions.includes("followers_only") && (
                <button
                  type="button"
                  className={styles.postBtnVariant}
                  disabled={posting || !text.trim() || overLimit}
                  onClick={() => submitWithVisibility("followers_only")}
                >
                  <span aria-hidden="true">
                    <TwemojiEmoji emoji="🔒️" />
                  </span>
                  {t("home:postComposer.postButtonPrivate")}
                </button>
              )}
            </div>
          ) : (
            <div className={styles.postBtnGroup}>
              <button
                type="button"
                className={styles.postBtnVariant}
                disabled={
                  posting || !text.trim() || overLimit || quoteTo?.visibility === "unlisted"
                }
                title={
                  quoteTo?.visibility === "unlisted"
                    ? t("home:postComposer.quoteUnlistedPublicConflict")
                    : undefined
                }
                onClick={() => submitWithVisibility("public")}
              >
                <span aria-hidden="true">
                  <TwemojiEmoji emoji="🌐" />
                </span>
                {t("home:postComposer.postButtonPublic")}
              </button>
              <button
                type="button"
                className={styles.postBtnVariant}
                disabled={posting || !text.trim() || overLimit}
                onClick={() => submitWithVisibility("unlisted")}
              >
                <span aria-hidden="true">
                  <TwemojiEmoji emoji="🌙" />
                </span>
                {t("home:postComposer.postButtonUnlisted")}
              </button>
              <div
                className={styles.postBtnTooltipWrap}
                onMouseEnter={handlePrivateTooltipEnter}
                onMouseLeave={handlePrivateTooltipLeave}
                onClick={handlePrivateTooltipClick}
              >
                <button
                  type="button"
                  className={styles.postBtnVariant}
                  disabled={posting || !text.trim() || overLimit || deliverBsky}
                  onClick={() => submitWithVisibility("followers_only")}
                >
                  <span aria-hidden="true">
                    <TwemojiEmoji emoji="🔒️" />
                  </span>
                  {t("home:postComposer.postButtonPrivate")}
                </button>
                {showPrivateTooltip && (
                  <span className={styles.popoverBelow} role="status">
                    <img className={styles.popoverBskyIcon} src={blueskyLogo} alt="" />
                    {t("home:postComposer.bskyOffToPrivateHint")}
                  </span>
                )}
              </div>
            </div>
          )}
        </div>
      </div>
    </form>
  );
}
