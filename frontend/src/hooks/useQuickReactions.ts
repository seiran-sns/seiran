import { useEffect, useState } from "react";
import { api, FrequentReaction } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import { fetchCustomEmojis } from "../lib/customEmojis";
import { QUICK_REACTION_HEART, QuickReaction, selectQuickReactions } from "../lib/quickReactions";

const HEART_ONLY: QuickReaction[] = [{ content: QUICK_REACTION_HEART }];

/** 自分のリアクション後に候補を取り直すまでの待ち時間。連続して付けたときに1回にまとめる。 */
const REFRESH_DELAY_MS = 2000;

// タイムラインの全 NoteCard が同じ候補を使うため、プロセス内で1つだけ持って購読させる
// （カードごとに `GET /api/reactions/frequent` を叩かない）。ログインユーザーが替わったら取り直す。
let current: QuickReaction[] = HEART_ONLY;
let loadedFor: string | null = null;
let refreshTimer: number | null = null;
const listeners = new Set<(items: QuickReaction[]) => void>();

function publish(items: QuickReaction[]) {
  current = items;
  listeners.forEach((l) => l(items));
}

function load(userId: string) {
  loadedFor = userId;
  Promise.all([
    api.reactions.frequent().catch(() => ({ items: [] as FrequentReaction[] })),
    fetchCustomEmojis().catch(() => []),
  ]).then(([frequent, customEmojis]) => {
    if (loadedFor !== userId) return;
    publish(selectQuickReactions(frequent.items, customEmojis));
  });
}

/** 自分のリアクションが変わったときに呼ぶ。少し待ってから「よく使う絵文字」を取り直す。 */
export function scheduleQuickReactionsRefresh() {
  const userId = loadedFor;
  if (userId === null) return;
  if (refreshTimer !== null) window.clearTimeout(refreshTimer);
  refreshTimer = window.setTimeout(() => {
    refreshTimer = null;
    if (loadedFor === userId) load(userId);
  }, REFRESH_DELAY_MS);
}

/** NoteCard のクイックリアクションボタンに並べる絵文字（先頭は常に ❤️）。 */
export function useQuickReactions(): QuickReaction[] {
  const { user } = useAuth();
  const userId = user ? String(user.id) : null;
  const [items, setItems] = useState(current);

  useEffect(() => {
    listeners.add(setItems);
    if (userId === null) {
      if (loadedFor !== null) {
        loadedFor = null;
        publish(HEART_ONLY);
      }
    } else if (loadedFor !== userId) {
      publish(HEART_ONLY);
      load(userId);
    }
    setItems(current);
    return () => {
      listeners.delete(setItems);
    };
  }, [userId]);

  return items;
}
