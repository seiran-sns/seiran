import { useAuth } from "../contexts/AuthContext";
import { api } from "../api/client";

/** 非表示にできるガイドのキー（バックエンド`seiran_common::guides::HIDEABLE_GUIDES`と同じ）。 */
export const HIDEABLE_GUIDES = ["redraft_note"] as const;
export type GuideKey = (typeof HIDEABLE_GUIDES)[number];

/**
 * 「今後は表示しない」にしたガイドの状態（`users.hidden_guides`）。
 * 保存に成功するとログイン中ユーザーの情報へ即反映する（失敗時は例外を投げる）。
 */
export function useHiddenGuides() {
  const { user, updateUser } = useAuth();
  const hiddenGuides = user?.hidden_guides ?? [];

  async function setGuideHidden(guide: GuideKey, hidden: boolean) {
    const result = await api.account.updateHiddenGuide(guide, hidden);
    updateUser({ hidden_guides: result.hidden_guides });
  }

  return {
    isGuideHidden: (guide: GuideKey) => hiddenGuides.includes(guide),
    setGuideHidden,
  };
}
