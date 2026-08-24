import { useTranslation } from "react-i18next";
import { pollRemainingTime } from "../../lib/format";
import { useSecondTicker } from "../../stores/secondTicker";

interface PollCountdownProps {
  endTime: string;
  className?: string;
}

/**
 * アンケート期限までの残り時間を表示し、1秒ごとにカウントダウンする（#228）。
 * `useSecondTicker`（共有の1秒タイマーストア）を購読するため、この小コンポーネントが
 * マウントされている間だけ1秒ごとに再レンダリングされる（`NoteCard`全体は再レンダリング
 * されない）。期限切れになった時点で自動的に非表示になる。
 */
export default function PollCountdown({ endTime, className }: PollCountdownProps) {
  const { t } = useTranslation();
  const now = useSecondTicker();
  const remaining = pollRemainingTime(endTime, now);
  if (!remaining) return null;
  const label = (() => {
    switch (remaining.tier) {
      case "seconds":
        return t("home:noteCard.pollRemainingSeconds", { count: remaining.seconds });
      case "minutes":
        return t("home:noteCard.pollRemainingMinutes", { count: remaining.minutes });
      case "hours":
        return t("home:noteCard.pollRemainingHours", {
          hours: remaining.hours,
          minutes: remaining.minutes,
        });
      case "days":
        return t("home:noteCard.pollRemainingDays", {
          days: remaining.days,
          hours: remaining.hours,
          minutes: remaining.minutes,
        });
    }
  })();
  return <span className={className}>{label}</span>;
}
