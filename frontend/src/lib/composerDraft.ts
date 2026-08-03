import { DriveFile } from "../api/client";

export type DraftVisibility = "public" | "unlisted" | "followers_only";

export interface ComposerDraft {
  text: string;
  attached: DriveFile | null;
  deliverFedi: boolean;
  deliverBsky: boolean;
  visibility: DraftVisibility;
}

export type DraftTarget =
  | { mode: "compose"; userId: number }
  | { mode: "reply"; userId: number; postId: string }
  | { mode: "quote"; userId: number; postId: string };

/** 返信・引用の書きかけをユーザーごとに何件まで保持するか（#193）。 */
const MAX_TARGETED_DRAFTS = 10;

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
  if (!draft.text.trim() && !draft.attached) {
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
