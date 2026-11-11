import { useEffect, useState } from "react";
import { api, FrequentReaction } from "../api/client";
import { useAuth } from "../contexts/AuthContext";
import { fetchCustomEmojis } from "../lib/customEmojis";
import { mergeQuickReactionsStable, QuickReaction, selectQuickReactions } from "../lib/quickReactions";
import { currentUtcOffsetMinutes, localTimeSlotKey, msUntilNextTimeSlot } from "../lib/reactionTimeSlot";

const NONE: QuickReaction[] = [];

/** 自分のリアクション後に候補を取り直すまでの待ち時間。連続して付けたときに1回にまとめる。 */
const REFRESH_DELAY_MS = 2000;

/** 時間帯の境目ちょうどに取り直すと、端末とサーバーの時計の差で前の時間帯の集計になりうるので少し遅らせる。 */
const TIME_SLOT_MARGIN_MS = 5000;

// タイムラインの全 NoteCard が同じ候補を使うため、プロセス内で1つだけ持って購読させる
// （カードごとに `GET /api/reactions/frequent` を叩かない）。ログインユーザーが替わったら取り直す。
let current: QuickReaction[] = NONE;
let loadedFor: string | null = null;
/** 今の候補をどの時間帯（`localTimeSlotKey`）のランキングで並べたか。 */
let loadedSlot: string | null = null;
let refreshTimer: number | null = null;
let slotTimer: number | null = null;
const listeners = new Set<(items: QuickReaction[]) => void>();

function publish(items: QuickReaction[]) {
  current = items;
  listeners.forEach((l) => l(items));
}

/**
 * 今の時間帯のランキングで取り直す。`refresh`（自分のリアクション後の取り直し）では表示中の並びを
 * 保つ（`mergeQuickReactionsStable`）。ただし時間帯が変わっていれば別のランキングなので、
 * 新しい時間帯の順位で並べ直す。
 */
function load(userId: string, refresh = false) {
  loadedFor = userId;
  const slot = localTimeSlotKey();
  Promise.all([
    api.reactions
      .frequent(currentUtcOffsetMinutes())
      .catch(() => ({ items: [] as FrequentReaction[] })),
    fetchCustomEmojis().catch(() => []),
  ]).then(([frequent, customEmojis]) => {
    if (loadedFor !== userId) return;
    const next = selectQuickReactions(frequent.items, customEmojis);
    const keepPositions = refresh && loadedSlot === slot;
    loadedSlot = slot;
    publish(keepPositions ? mergeQuickReactionsStable(current, next) : next);
  });
  scheduleTimeSlotReload(userId);
}

/** 次の時間帯の境目で取り直す（タブを開いたままでも時間帯ごとのランキングに切り替わるように）。 */
function scheduleTimeSlotReload(userId: string) {
  if (slotTimer !== null) window.clearTimeout(slotTimer);
  slotTimer = window.setTimeout(() => {
    slotTimer = null;
    if (loadedFor === userId) load(userId);
  }, msUntilNextTimeSlot() + TIME_SLOT_MARGIN_MS);
}

// スリープ等でタイマーが遅れても、タブに戻ったときに時間帯が変わっていれば取り直す。
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible" || loadedFor === null) return;
    if (loadedSlot !== null && loadedSlot !== localTimeSlotKey()) load(loadedFor);
  });
}

/** 自分のリアクションが変わったときに呼ぶ。少し待ってから「よく使う絵文字」を取り直す。 */
export function scheduleQuickReactionsRefresh() {
  const userId = loadedFor;
  if (userId === null) return;
  if (refreshTimer !== null) window.clearTimeout(refreshTimer);
  refreshTimer = window.setTimeout(() => {
    refreshTimer = null;
    if (loadedFor === userId) load(userId, true);
  }, REFRESH_DELAY_MS);
}

/** NoteCard のクイックリアクションのポップアップに並べる絵文字（❤️を除くよく使う順）。 */
export function useQuickReactions(): QuickReaction[] {
  const { user } = useAuth();
  const userId = user ? String(user.id) : null;
  const [items, setItems] = useState(current);

  useEffect(() => {
    listeners.add(setItems);
    if (userId === null) {
      if (loadedFor !== null) {
        loadedFor = null;
        loadedSlot = null;
        if (slotTimer !== null) window.clearTimeout(slotTimer);
        slotTimer = null;
        publish(NONE);
      }
    } else if (loadedFor !== userId) {
      publish(NONE);
      load(userId);
    }
    setItems(current);
    return () => {
      listeners.delete(setItems);
    };
  }, [userId]);

  return items;
}
