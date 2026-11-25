import type { ScheduledPost } from "../api/client";
import type { ComposerDraft } from "./composerDraft";
import i18n from "../i18n";

/** `<input type="datetime-local">` の値（端末のタイムゾーンでの `YYYY-MM-DDTHH:mm`）。 */
export function toDatetimeLocal(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** 予約日時を新しく指定するときの初期値（1時間後を5分単位に切り上げた時刻）。 */
export function defaultScheduleValue(now: Date = new Date()): string {
  const t = new Date(now.getTime() + 60 * 60 * 1000);
  t.setSeconds(0, 0);
  t.setMinutes(Math.ceil(t.getMinutes() / 5) * 5);
  return toDatetimeLocal(t);
}

/** 予約日時に指定できる最も早い時刻（バックエンドの `scheduled_posts::MIN_LEAD` と同じ5分後）。 */
export function minScheduleValue(now: Date = new Date()): string {
  return toDatetimeLocal(new Date(now.getTime() + 5 * 60 * 1000));
}

/** 予約の内容を投稿欄の状態にする（予約の編集）。 */
export function draftFromScheduledPost(post: ScheduledPost): ComposerDraft {
  const req = post.request;
  const poll = req.poll;
  const pollExpiry: ComposerDraft["pollExpiry"] = poll?.expiresAtIso
    ? { kind: "at", value: toDatetimeLocal(new Date(poll.expiresAtIso)) }
    : poll?.expiresAt
      ? { kind: "at", value: toDatetimeLocal(new Date(poll.expiresAt)) }
      : poll?.expiresInSeconds
        ? { kind: "duration", seconds: poll.expiresInSeconds }
        : poll?.expiredAfter
          ? { kind: "duration", seconds: Math.round(poll.expiredAfter / 1000) }
          : { kind: "none" };
  const sensitive = new Set(req.sensitive_attachment_ids ?? []);
  const visibility = req.visibility;
  return {
    text: req.text ?? "",
    attachments: post.attachments.map((a) => ({
      ...a,
      isSensitive: req.sensitive_attachment_ids ? sensitive.has(a.id) : a.isSensitive,
    })),
    deliverFedi: req.deliver_to_fedi ?? true,
    deliverBsky: req.deliver_to_bsky ?? true,
    bskyEmbedChoice: req.bsky_embed_choice ?? null,
    pollEnabled: !!poll,
    pollChoices: poll?.choices ?? ["", ""],
    pollMultiple: poll?.multiple ?? false,
    pollExpiry,
    cwEnabled: !!req.content_warning,
    cwGuide: req.content_warning ?? "",
    linkCardUrls: req.link_card_urls ?? [],
    language: req.language ?? "",
    ...(visibility && visibility !== "direct" ? { visibility } : {}),
  };
}

/** 投稿できなかった予約の理由（エラーコード）の表示文。 */
export function scheduledFailureMessage(code: string): string {
  const key = `errors:${code}`;
  return i18n.exists(key) ? i18n.t(key) : i18n.t("errors:UNKNOWN_WITH_CODE", { code });
}
