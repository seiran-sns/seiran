/**
 * 「よく使う絵文字」の時間帯（1日を閲覧者の地域の0時から4時間ずつ6つに分けたもの）まわり。
 * サーバー（`GET /api/reactions/frequent?utcOffset=`）はタイムゾーン名を扱わず、ブラウザが
 * 今のUTCオフセット（分）を渡して整数演算だけで時間帯を決める。
 */

/** 1つの時間帯の長さ（時間）。サーバーの集計（`aggregate_for_actor`）と揃える。 */
const TIME_SLOT_HOURS = 4;

/** ブラウザの今のUTCオフセット（分、東が正。日本なら540、インドなら330）。 */
export function currentUtcOffsetMinutes(now: Date = new Date()): number {
  // `getTimezoneOffset` は「UTC − 現地」なので符号が逆。`-0` を避けて 0 にそろえる。
  return -now.getTimezoneOffset() || 0;
}

/** 現地の日付と時間帯で、時間帯が変わったかを見分けるためのキー。 */
export function localTimeSlotKey(now: Date = new Date()): string {
  const slot = Math.floor(now.getHours() / TIME_SLOT_HOURS);
  return `${now.getFullYear()}-${now.getMonth()}-${now.getDate()}:${slot}`;
}

/** 次の時間帯の境目（現地の0時・4時・8時…）までのミリ秒。夏時間の切り替えは `Date` に任せる。 */
export function msUntilNextTimeSlot(now: Date = new Date()): number {
  const nextHour = (Math.floor(now.getHours() / TIME_SLOT_HOURS) + 1) * TIME_SLOT_HOURS;
  const next = new Date(now.getFullYear(), now.getMonth(), now.getDate(), nextHour, 0, 0, 0);
  return Math.max(0, next.getTime() - now.getTime());
}
