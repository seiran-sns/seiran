import { ChangeEvent, FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, BskyEmbedChoice, DriveFile, Note, getErrorMessage } from "../../api/client";
import { acct, calcRemaining, displayName, extractBodyUrls } from "../../lib/format";
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

/** 添付ファイルの最大件数（バックエンド`validate_attachment_ids`と同じ上限）。 */
const MAX_ATTACHMENTS = 10;

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

/** Bsky embed候補1件（ラジオボタンリストの1アイテム）。 */
interface EmbedCandidate {
  key: string;
  choice: BskyEmbedChoice;
  label: string;
  /** アニメGIF・動画/音声候補のみ。「動画1」等の表記だけではどれか分からないため
   * 小さなサムネイルを添える（マイケル指摘）。動画/音声はサムネイル抽出に失敗している
   * 場合があり、その場合は無し。 */
  thumbnailUrl?: string;
}

function embedChoiceKey(choice: BskyEmbedChoice): string {
  switch (choice.kind) {
    case "images":
      return "images";
    case "attachment":
      return `attachment:${choice.id}`;
    case "url":
      return `url:${choice.url}`;
  }
}

/**
 * 添付・本文URLから、Bsky embed候補一覧を組み立てる（#227）。静止画は非アニメ画像が
 * 1件でもあれば全体で1アイテム（Bsky embed.imagesは最大4枚を1つのembedとして送る）、
 * アニメGIF・動画/音声・本文URLはそれぞれ1件ずつ独立したアイテムになる。
 */
function buildEmbedCandidates(
  attachments: DriveFile[],
  bodyUrls: string[],
  t: (key: string, opts?: Record<string, unknown>) => string,
): EmbedCandidate[] {
  const images = attachments.filter(
    (a) => !a.isAnimatedImage && a.mimeType.startsWith("image/"),
  );
  const gifs = attachments.filter((a) => a.isAnimatedImage);
  const videos = attachments.filter(
    (a) => a.mimeType.startsWith("video/") || a.mimeType.startsWith("audio/"),
  );

  const candidates: EmbedCandidate[] = [];
  if (images.length > 0) {
    candidates.push({
      key: "images",
      choice: { kind: "images" },
      label: t("home:postComposer.bskyEmbedChoice.images", { count: images.length }),
    });
  }
  gifs.forEach((a, i) => {
    candidates.push({
      key: `attachment:${a.id}`,
      choice: { kind: "attachment", id: a.id },
      label: t("home:postComposer.bskyEmbedChoice.gif", { index: i + 1 }),
      thumbnailUrl: a.thumbnailUrl ?? a.url,
    });
  });
  videos.forEach((a, i) => {
    candidates.push({
      key: `attachment:${a.id}`,
      choice: { kind: "attachment", id: a.id },
      label: t("home:postComposer.bskyEmbedChoice.video", { index: i + 1 }),
      thumbnailUrl: a.thumbnailUrl,
    });
  });
  bodyUrls.forEach((url, i) => {
    candidates.push({
      key: `url:${url}`,
      choice: { kind: "url", url },
      label: t("home:postComposer.bskyEmbedChoice.url", { index: i + 1, url }),
    });
  });
  return candidates;
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
  const [attachments, setAttachments] = useState<DriveFile[]>(
    initialDraft?.attachments ?? [],
  );
  const [bskyEmbedChoice, setBskyEmbedChoice] = useState<BskyEmbedChoice | null>(
    initialDraft?.bskyEmbedChoice ?? null,
  );
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
    saveComposerDraft(draftTarget, {
      text,
      attachments,
      deliverFedi,
      deliverBsky,
      visibility,
      bskyEmbedChoice,
    });
  }, [draftTarget, text, attachments, deliverFedi, deliverBsky, visibility, bskyEmbedChoice]);

  useEffect(() => {
    if (!draftTarget) return;
    return onComposerDraftRefresh((target) => {
      if (JSON.stringify(target) !== JSON.stringify(draftTarget)) return;
      const draft = loadComposerDraft(draftTarget);
      setText(draft?.text ?? "");
      setAttachments(draft?.attachments ?? []);
      setBskyEmbedChoice(draft?.bskyEmbedChoice ?? null);
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

  const bodyUrls = useMemo(() => extractBodyUrls(text), [text]);
  const embedCandidates = useMemo(
    () => buildEmbedCandidates(attachments, bodyUrls, t),
    [attachments, bodyUrls, t],
  );
  // URL選択は本文からそのURLを削除しても選択自体は孤児として有効なまま残す（issue #227
  // 仕様）。他の選択肢へ切り替えれば自然にこのリストから消える。静止画/GIF/動画の選択は、
  // その添付自体が削除されると（下のuseEffectで）選択ごとクリアするため孤児化しない。
  const displayCandidates = useMemo(() => {
    if (
      bskyEmbedChoice?.kind === "url" &&
      !embedCandidates.some((c) => c.key === embedChoiceKey(bskyEmbedChoice))
    ) {
      return [
        ...embedCandidates,
        {
          key: embedChoiceKey(bskyEmbedChoice),
          choice: bskyEmbedChoice,
          label: t("home:postComposer.bskyEmbedChoice.urlOrphan", {
            url: bskyEmbedChoice.url,
          }),
        },
      ];
    }
    return embedCandidates;
  }, [embedCandidates, bskyEmbedChoice, t]);

  // 添付が無く本文URLも無い状態からURLリンクカードを添付する唯一の手段は、一旦本文へURLを
  // 書いてラジオボタンリストからそれを選び、その後本文からURLを削除する（孤児化）という
  // Blueskyの「URL貼り付け→プレビューカード→本文から消してもカードは残る」に近い操作。
  // これを可能にするため、候補が1件でも「URL関連（本文URLがある、または既にURL選択済み）」
  // なら曖昧さの有無に関わらずリストを表示する（マイケル指摘）。
  const hasUrlCandidate = bodyUrls.length > 0 || bskyEmbedChoice?.kind === "url";
  const showEmbedChoiceList = deliverBsky && (displayCandidates.length >= 2 || hasUrlCandidate);
  // 送信ブロックは「候補が2件以上あるのに未選択」という本当の曖昧さがある場合のみ（候補が
  // URL単独1件だけの場合は選ばなくても送信でき、その場合バックエンドの自動優先順位が
  // そのURLをそのまま採用する）。
  const embedChoiceMissing = deliverBsky && displayCandidates.length >= 2 && !bskyEmbedChoice;

  // Bsky配送オフ、または選択済みの静止画/GIF/動画がその添付自体の削除で候補から
  // 消えた場合は選択をクリアする（URL選択のみ孤児として残す、上記参照）。
  useEffect(() => {
    if (!deliverBsky) {
      if (bskyEmbedChoice !== null) setBskyEmbedChoice(null);
      return;
    }
    if (!bskyEmbedChoice || bskyEmbedChoice.kind === "url") return;
    if (!embedCandidates.some((c) => c.key === embedChoiceKey(bskyEmbedChoice))) {
      setBskyEmbedChoice(null);
    }
  }, [deliverBsky, embedCandidates, bskyEmbedChoice]);

  // 返信で表示する公開範囲ボタン。親から狭める方向のみ（replyVisibilityConstraint）、
  // かつ Bsky 配送中は followers_only を除く（プロトコル上フォロワー限定配信ができない
  // ため。新規投稿・引用は常に3段階全て表示し、followers_only はグレーアウト＋ツール
  // チップで理由を説明する既存方式のまま。返信は選択肢が親から動的に絞られるため、
  // 常にグレーアウトされ続けるボタンを出すより非表示にする方が分かりやすい）。
  const replyVisibilityOptions: Visibility[] = (
    replyConstraint?.options ?? []
  ).filter((v) => v !== "followers_only" || !deliverBsky);

  async function submitWithVisibility(v: Visibility) {
    if (!text.trim() || overLimit || posting || embedChoiceMissing) return;
    setError("");
    setPosting(true);
    try {
      const attachmentIds = attachments.map((a) => a.id);
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
        bskyEmbedChoice ?? undefined,
      );
      setText("");
      setAttachments([]);
      setBskyEmbedChoice(null);
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
      const uploaded = await api.media.upload(file);
      setAttachments((prev) => [...prev, uploaded]);
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setUploading(false);
    }
  }

  async function uploadFiles(files: File[]) {
    for (const file of files) {
      // 直列アップロード: 都度の残り枠を正しく見るため（並列だと枠超過チェックがずれる）。
      if (attachments.length + 1 > MAX_ATTACHMENTS) break;
      await uploadFile(file);
    }
  }

  function handleFileSelect(e: ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? []);
    e.target.value = "";
    if (files.length === 0) return;
    uploadFiles(files.slice(0, Math.max(0, MAX_ATTACHMENTS - attachments.length)));
  }

  function removeAttachment(id: string) {
    setAttachments((prev) => prev.filter((a) => a.id !== id));
  }

  const attachLimitReached = attachments.length >= MAX_ATTACHMENTS;

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
        multiple
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
          if (!uploading && !attachLimitReached) uploadFile(file);
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
          disabled={uploading || attachLimitReached}
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
        {attachments.length > 0 && (
          <div className={styles.attachGrid}>
            {attachments.map((attached) => (
              <div key={attached.id} className={styles.attachItem}>
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
                  onClick={() => removeAttachment(attached.id)}
                  title={t("home:postComposer.removeAttachmentTitle")}
                >
                  ×
                </button>
              </div>
            ))}
          </div>
        )}

        {showEmbedChoiceList && (
          <div className={styles.embedChoiceList} role="radiogroup" aria-label={t("home:postComposer.bskyEmbedChoice.heading")}>
            <p className={styles.embedChoiceHeading}>
              <img className={styles.embedChoiceBskyIcon} src={blueskyLogo} alt="" />
              {t("home:postComposer.bskyEmbedChoice.heading")}
            </p>
            {displayCandidates.map((candidate) => (
              <label key={candidate.key} className={styles.embedChoiceItem}>
                <input
                  type="radio"
                  name="bskyEmbedChoice"
                  checked={
                    bskyEmbedChoice !== null &&
                    embedChoiceKey(bskyEmbedChoice) === candidate.key
                  }
                  onChange={() => setBskyEmbedChoice(candidate.choice)}
                />
                {candidate.thumbnailUrl && (
                  <img
                    src={candidate.thumbnailUrl}
                    alt=""
                    className={styles.embedChoiceThumb}
                  />
                )}
                {candidate.label}
              </label>
            ))}
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
                  disabled={posting || !text.trim() || overLimit || embedChoiceMissing}
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
                  disabled={posting || !text.trim() || overLimit || embedChoiceMissing}
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
                  disabled={posting || !text.trim() || overLimit || embedChoiceMissing}
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
                  posting ||
                  !text.trim() ||
                  overLimit ||
                  embedChoiceMissing ||
                  quoteTo?.visibility === "unlisted"
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
                disabled={posting || !text.trim() || overLimit || embedChoiceMissing}
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
