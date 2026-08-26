export interface ComposerDefaults {
  visibility: "public" | "unlisted" | "followers_only";
  deliverFedi: boolean;
  deliverBsky: boolean;
}

const KEY = "seiran:composer-defaults";

/** 直近に送信した新規投稿・引用の公開範囲・配送先設定。次回の新規投稿・引用のデフォルト
 * ボタン（Ctrl+Enter等のショートカット送信先）として使う。返信は親ポストから決まる専用の
 * デフォルト（`replyVisibilityConstraint`）を持つため対象外。 */
export function loadComposerDefaults(): ComposerDefaults | null {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? (JSON.parse(raw) as ComposerDefaults) : null;
  } catch {
    return null;
  }
}

export function saveComposerDefaults(defaults: ComposerDefaults): void {
  localStorage.setItem(KEY, JSON.stringify(defaults));
}
