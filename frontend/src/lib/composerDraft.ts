import { BskyEmbedChoice, DriveFile } from "../api/client";

/** アンケートの期限指定（#228）。`PostComposer`のpollExpiry stateと同じ形。 */
export type DraftPollExpiry =
  | { kind: "none" }
  | { kind: "at"; value: string }
  | { kind: "duration"; seconds: number };

export interface ComposerDraft {
  text: string;
  attachments: DriveFile[];
  deliverFedi: boolean;
  deliverBsky: boolean;
  /** Bsky embed選択（#227）。候補が2件以上ある間の選択中の値、または孤児化したURL選択。 */
  bskyEmbedChoice: BskyEmbedChoice | null;
  /** アンケート編集中かどうか（#228）。 */
  pollEnabled: boolean;
  pollChoices: string[];
  pollMultiple: boolean;
  pollExpiry: DraftPollExpiry;
  /** CW（閲覧注意）編集中かどうか（#229）。 */
  cwEnabled: boolean;
  cwGuide: string;
  /** URLリンクカード添付のチェックボックス選択（Bsky embed選択のラジオボタンリストを
   * 出せない場合の代替、Bsky配送オフ or CW中）。本文から消えてもチェック自体は
   * 孤児として残る（`bskyEmbedChoice`のURL孤児化と同じ仕様）。 */
  linkCardUrls: string[];
}

export type DraftTarget =
  | { mode: "compose"; userId: number }
  | { mode: "reply"; userId: number; postId: string }
  | { mode: "quote"; userId: number; postId: string };

/** 返信・引用の書きかけをユーザーごとに何件まで保持するか（#193）。 */
const MAX_TARGETED_DRAFTS = 10;
const DRAFT_REFRESH_EVENT = "seiran:composer-draft-refresh";

function draftKey(target: DraftTarget): string {
  switch (target.mode) {
    case "compose":
      return `seiran:draft:compose:${target.userId}`;
    case "reply":
      return `seiran:draft:reply:${target.userId}:${target.postId}`;
    case "quote":
      return `seiran:draft:quote:${target.userId}:${target.postId}`;
  }
}

function indexKey(target: Extract<DraftTarget, { mode: "reply" | "quote" }>): string {
  return `seiran:draft:${target.mode}-index:${target.userId}`;
}

function readIndex(key: string): string[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(key) ?? "[]");
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

/** postIdを最新扱いにして末尾へ移動し、上限を超えた最古の下書きを削除する。 */
function touchIndex(target: Extract<DraftTarget, { mode: "reply" | "quote" }>): void {
  const key = indexKey(target);
  const ids = readIndex(key).filter((id) => id !== target.postId);
  ids.push(target.postId);
  while (ids.length > MAX_TARGETED_DRAFTS) {
    const oldest = ids.shift();
    if (oldest !== undefined) {
    localStorage.removeItem(draftKey({ ...target, postId: oldest }));
    }
  }
  localStorage.setItem(key, JSON.stringify(ids));
}

function removeFromIndex(target: Extract<DraftTarget, { mode: "reply" | "quote" }>): void {
  const key = indexKey(target);
  const ids = readIndex(key).filter((id) => id !== target.postId);
  localStorage.setItem(key, JSON.stringify(ids));
}

export function loadComposerDraft(target: DraftTarget): ComposerDraft | null {
  try {
  const raw = localStorage.getItem(draftKey(target));
    return raw ? (JSON.parse(raw) as ComposerDraft) : null;
  } catch {
    return null;
  }
}

/** 本文・添付とも空の下書きは保存せず、既存の下書きがあれば消す（クリア相当）。 */
export function saveComposerDraft(target: DraftTarget, draft: ComposerDraft): void {
  if (
    !draft.text.trim() &&
    draft.attachments.length === 0 &&
    !draft.pollEnabled &&
    !draft.cwEnabled &&
    draft.linkCardUrls.length === 0
  ) {
    clearComposerDraft(target);
    return;
  }
  localStorage.setItem(draftKey(target), JSON.stringify(draft));
  if (target.mode !== "compose") touchIndex(target);
}

export function clearComposerDraft(target: DraftTarget): void {
  localStorage.removeItem(draftKey(target));
  if (target.mode !== "compose") removeFromIndex(target);
}

/** 既にマウント済みの同一対象コンポーザへ、localStorageの再読込を依頼する。 */
export function refreshComposerDraft(target: DraftTarget): void {
  window.dispatchEvent(new CustomEvent(DRAFT_REFRESH_EVENT, { detail: target }));
}

export function onComposerDraftRefresh(
  listener: (target: DraftTarget) => void,
): () => void {
  const handler = (event: Event) => listener((event as CustomEvent<DraftTarget>).detail);
  window.addEventListener(DRAFT_REFRESH_EVENT, handler);
  return () => window.removeEventListener(DRAFT_REFRESH_EVENT, handler);
}
