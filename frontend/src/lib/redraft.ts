import { DriveFile, Note, NoteAttachment } from "../api/client";
import { ComposerDraft, DraftTarget } from "./composerDraft";

/** 「削除して編集再投稿」を選べない理由。`null`なら選べる。 */
export type RedraftBlockReason =
  | "replyUnavailable"
  | "quoteUnavailable"
  | "replyAndQuote"
  | "attachmentUnavailable";

/**
 * 削除すると、元と同じ内容で投稿し直せなくなる場合の理由を返す。
 * 返信元・引用元が既に無いと、再投稿の宛先を解決できずコンポーザを開けない
 * （削除してから失敗すると、ポストだけが失われる）。返信かつ引用のポストは
 * コンポーザが表現できない。`id`の無い添付（リモート由来）は再投稿に引き継げない。
 */
export function redraftBlockReason(note: Note): RedraftBlockReason | null {
  if (note.replyStatus) return "replyUnavailable";
  if (note.quoteStatus) return "quoteUnavailable";
  if (note.replyId && note.quoteId) return "replyAndQuote";
  if (note.attachments.some((a) => !a.id)) return "attachmentUnavailable";
  return null;
}

/** 添付をコンポーザの`DriveFile`へ。ここで使うのはid・表示用のURL・種別だけで、
 * 重複排除用のsha256・サイズは再投稿に不要。 */
function toDriveFile(a: NoteAttachment): DriveFile {
  return {
    id: a.id as string,
    url: a.url,
    sha256: "",
    width: a.width || undefined,
    height: a.height || undefined,
    size: 0,
    mimeType: a.mimeType,
    isReused: false,
    durationMs: a.durationMs,
    thumbnailUrl: a.thumbnailUrl,
    isAnimatedImage: a.isAnimatedImage,
  };
}

/** `<input type="datetime-local">`が受け取るローカル時刻の文字列（`YYYY-MM-DDTHH:mm`）。 */
function toDatetimeLocal(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

/** 再投稿の下書きを保存・読み込みするキー。返信・引用は元ポストをキーにする（宛先ごとに
 * 下書きを持つ`composerDraft`の仕組みにそのまま乗る）。 */
export function redraftDraftTarget(note: Note, userId: number): DraftTarget {
  if (note.replyId) return { mode: "reply", userId, postId: note.replyId };
  if (note.quoteId) return { mode: "quote", userId, postId: note.quoteId };
  return { mode: "compose", userId };
}

/** 削除するポストの内容を、コンポーザの下書きへ写す。`now`は終了済みアンケートの判定用。 */
export function buildRedraftDraft(note: Note, now: Date = new Date()): ComposerDraft {
  const poll = note.poll;
  const pollEnd = poll?.endTime ? new Date(poll.endTime) : null;
  const visibility =
    note.visibility === "unlisted" || note.visibility === "followers_only"
      ? note.visibility
      : "public";
  return {
    text: note.text,
    attachments: note.attachments.map(toDriveFile),
    deliverFedi: note.deliverFedi ?? true,
    deliverBsky: note.deliverBsky ?? true,
    bskyEmbedChoice: null,
    pollEnabled: !!poll,
    pollChoices: poll ? poll.options.map((o) => o.name) : ["", ""],
    pollMultiple: poll?.multiple ?? false,
    // 終了済みの期限は再投稿できないので、期限なしに戻す。
    pollExpiry:
      pollEnd && pollEnd > now
        ? { kind: "at", value: toDatetimeLocal(pollEnd) }
        : { kind: "none" },
    cwEnabled: !!note.contentWarning,
    cwGuide: note.contentWarning ?? "",
    linkCardUrls: note.linkCards.map((c) => c.url),
    language: "",
    visibility,
  };
}
