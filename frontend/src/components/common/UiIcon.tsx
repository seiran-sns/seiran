import { ReactNode } from "react";
import { useLowContrast } from "../../stores/lowContrastStore";
import TwemojiEmoji from "./TwemojiEmoji";
import styles from "./UiIcon.module.css";

/**
 * 「落ち着いたアイコン」の線画（24×24、塗り無し・`currentColor` の線だけ）。キーはアイコンに使っている
 * 絵文字から異体字セレクタ（U+FE0F）を除いたもの。ここに無い絵文字は設定に関わらず Twemoji で描く。
 */
const LINE_ICONS: Record<string, ReactNode> = {
  // メニュー
  "🏠": (
    <>
      <path d="M3.5 10.5 12 3.5l8.5 7" />
      <path d="M5.5 9v11h4.5v-6h4v6h4.5V9" />
    </>
  ),
  "🔍": (
    <>
      <circle cx="10.5" cy="10.5" r="6.5" />
      <path d="m15.5 15.5 5 5" />
    </>
  ),
  "🔔": (
    <>
      <path d="M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 1.5h-15z" />
      <path d="M10 20.5a2.2 2.2 0 0 0 4 0" />
    </>
  ),
  "✉": (
    <>
      <rect x="3" y="5.5" width="18" height="13" rx="2" />
      <path d="m3.8 6.8 8.2 6.5 8.2-6.5" />
    </>
  ),
  "📋": (
    <>
      <rect x="5" y="4.5" width="14" height="16.5" rx="2" />
      <rect x="9" y="2.8" width="6" height="3.4" rx="1" />
      <path d="M8.5 11h7M8.5 15h5" />
    </>
  ),
  "⚙": (
    <>
      <path d="M18.54 9.51 L21.17 10.43 L21.17 13.57 L18.54 14.49 L18.39 14.86 L19.59 17.37 L17.37 19.59 L14.86 18.39 L14.49 18.54 L13.57 21.17 L10.43 21.17 L9.51 18.54 L9.14 18.39 L6.63 19.59 L4.41 17.37 L5.61 14.86 L5.46 14.49 L2.83 13.57 L2.83 10.43 L5.46 9.51 L5.61 9.14 L4.41 6.63 L6.63 4.41 L9.14 5.61 L9.51 5.46 L10.43 2.83 L13.57 2.83 L14.49 5.46 L14.86 5.61 L17.37 4.41 L19.59 6.63 L18.39 9.14Z" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  "🛡": <path d="M12 3 4.5 6v5.5c0 4.6 3.2 8.2 7.5 9.5 4.3-1.3 7.5-4.9 7.5-9.5V6z" />,
  "🔗": (
    <>
      <path d="M10 14a4 4 0 0 0 5.7 0l3-3A4 4 0 0 0 13 5.3l-1 1" />
      <path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" />
    </>
  ),
  "✏": (
    <>
      <path d="M4 20l1.2-4.4L16 4.8a2 2 0 0 1 2.8 0l.4.4a2 2 0 0 1 0 2.8L8.4 18.8z" />
      <path d="m14.5 6.5 3 3" />
    </>
  ),
  "🔑": (
    <>
      <circle cx="8" cy="15.5" r="4" />
      <path d="m11 12.5 8.5-8.5M16.5 7l2.5 2.5M14.5 9l2 2" />
    </>
  ),
  "ℹ": (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v6M12 7.6v.2" />
    </>
  ),
  "☰": <path d="M4 6.5h16M4 12h16M4 17.5h16" />,
  // 通知欄の種別
  "➕": <path d="M12 4.5v15M4.5 12h15" />,
  "🙋": (
    <>
      <circle cx="10" cy="8" r="3.2" />
      <path d="M4 20.5c.4-3.6 2.8-6 6-6 1.6 0 3 .6 4 1.6M17.5 3.5v9M15 6l2.5-2.5L20 6" />
    </>
  ),
  "🤝": (
    <>
      <path d="M2.5 9.5 6 6.5l3.5 1.5L12 6.5l3 0 3.5 3" />
      <path d="M21.5 9.5 18.5 12.5l-6.5 6-1.5-1.5M9.5 8l3.5 4M6 6.5 2.5 9.5l3 3 5 5M8 15l1.5 1.5M10 13l2 2" />
    </>
  ),
  "📣": (
    <>
      <path d="M3.5 10v4h3l9 5V5l-9 5z" />
      <path d="M6.5 14l1.5 5.5h2.5L9.5 14.5M18.5 9.5a3 3 0 0 1 0 5" />
    </>
  ),
  // 設定メニュー
  "🔐": (
    <>
      <rect x="3" y="11" width="12" height="9.5" rx="1.8" />
      <path d="M5.8 11V8a3.2 3.2 0 0 1 6.4 0v3" />
      <circle cx="18.5" cy="8.5" r="2.3" />
      <path d="M18.5 10.8V20M18.5 15.5h2M18.5 18h2" />
    </>
  ),
  "🪪": (
    <>
      <rect x="3" y="5" width="18" height="14" rx="2" />
      <circle cx="8.5" cy="10.5" r="2" />
      <path d="M5.5 16c.5-1.7 1.6-2.6 3-2.6s2.5.9 3 2.6M14 10h4M14 14h4" />
    </>
  ),
  "🚫": (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="m5.7 5.7 12.6 12.6" />
    </>
  ),
  "🔒": (
    <>
      <rect x="5" y="10.5" width="14" height="10.5" rx="2" />
      <path d="M8 10.5V7.5a4 4 0 0 1 8 0v3M12 14.5v2.5" />
    </>
  ),
  "🔕": (
    <>
      <path d="M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 1.5h-15z" />
      <path d="M10 20.5a2.2 2.2 0 0 0 4 0M3.5 3.5l17 17" />
    </>
  ),
  "🎨": (
    <>
      <path d="M12 3a9 9 0 1 0 0 18c1.2 0 1.6-.9 1.2-1.7-.5-1-.2-2.5 1.4-2.5H17a4 4 0 0 0 4-4C21 7.2 17 3 12 3z" />
      <circle cx="7.5" cy="11.5" r="1.1" />
      <circle cx="10" cy="7.3" r="1.1" />
      <circle cx="15" cy="7.5" r="1.1" />
    </>
  ),
  "🚚": (
    <>
      <path d="M2.5 5.5h10.5v10.5H2.5zM13 9h4.3l3.2 3.5V16H13" />
      <circle cx="6.5" cy="17.5" r="1.8" />
      <circle cx="16.8" cy="17.5" r="1.8" />
    </>
  ),
  "👥": (
    <>
      <circle cx="9" cy="8" r="3.2" />
      <path d="M3 20c.5-3.5 2.8-5.5 6-5.5s5.5 2 6 5.5M15.5 4.9a3.2 3.2 0 0 1 0 6.2M17.5 14.6c2 .7 3.1 2.6 3.5 5.4" />
    </>
  ),
  // 投稿カードのボタンバー
  "💬": <path d="M5 4.5h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-8.5L6 20.5v-4H5a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2z" />,
  "🔁": (
    <>
      <path d="M4 11.5V9.5a3 3 0 0 1 3-3h12.5" />
      <path d="m16.5 3.5 3 3-3 3" />
      <path d="M20 12.5v2a3 3 0 0 1-3 3H4.5" />
      <path d="m7.5 20.5-3-3 3-3" />
    </>
  ),
  "🙂": (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M8.5 14.5a4 4 0 0 0 7 0M9 9.5v.6M15 9.5v.6" />
    </>
  ),
  "❤": (
    <path d="M12 20s-7.3-4.4-8.8-9.1C2.2 7.6 4.4 4.5 7.5 4.5c2 0 3.3 1.1 4.5 2.8 1.2-1.7 2.5-2.8 4.5-2.8 3.1 0 5.3 3.1 4.3 6.4C19.3 15.6 12 20 12 20z" />
  ),
  // 投稿フォームのボタンバー
  "⚠": (
    <>
      <path d="M10.3 4.5a2 2 0 0 1 3.4 0l7.6 13.2a2 2 0 0 1-1.7 3H4.4a2 2 0 0 1-1.7-3z" />
      <path d="M12 10v4.5M12 17.4v.2" />
    </>
  ),
  "📊": (
    <>
      <path d="M3.5 20.5h17" />
      <rect x="5.5" y="11" width="3" height="6.5" />
      <rect x="10.5" y="5.5" width="3" height="12" />
      <rect x="15.5" y="13.5" width="3" height="4" />
    </>
  ),
  "😀": (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M7.8 13h8.4a4.2 4.2 0 0 1-8.4 0zM9 9v.6M15 9v.6" />
    </>
  ),
  "🙈": (
    <>
      <path d="M2.5 12S6 6 12 6s9.5 6 9.5 6-3.5 6-9.5 6-9.5-6-9.5-6z" />
      <circle cx="12" cy="12" r="2.6" />
      <path d="M4 4l16 16" />
    </>
  ),
  "🌐": (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3c2.4 2.5 3.7 5.5 3.7 9s-1.3 6.5-3.7 9c-2.4-2.5-3.7-5.5-3.7-9S9.6 5.5 12 3z" />
    </>
  ),
  "🌙": <path d="M19.5 14.5A8 8 0 1 1 9.5 4.5a6.5 6.5 0 0 0 10 10z" />,
};

/** 線画のうち、意味を色で示すもの（リポスト＝緑、いいね＝赤）。色は `index.css` のトークン。 */
const LINE_ICON_COLORS: Record<string, string> = {
  "🔁": "var(--line-icon-repost)",
  "❤": "var(--line-icon-heart)",
};

function lineIconKey(emoji: string): string {
  return emoji.replace(/️/g, "");
}

interface UiIconProps {
  /** Twemoji で描くときの絵文字（線画の選択にも使う）。 */
  emoji: string;
  className?: string;
}

/**
 * メニュー・設定メニュー・投稿カード/投稿フォームのボタンバーに置く、操作を表すアイコン。通常は
 * Twemoji、低コントラストモード（`stores/lowContrastStore.ts`）がオンの端末ではモノクロの線画で描く
 * （カラフルなアイコンも刺激になるので、色づかいと一緒に落ち着かせる）。
 * 投稿本文やリアクションの絵文字（利用者が選んだ内容そのもの）には使わない。
 */
export default function UiIcon({ emoji, className }: UiIconProps) {
  const calm = useLowContrast();
  const key = lineIconKey(emoji);
  const shape = calm ? LINE_ICONS[key] : undefined;
  if (!shape) return <TwemojiEmoji emoji={emoji} className={className} />;
  return (
    <svg
      className={className ? `${styles.icon} ${className}` : styles.icon}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      style={LINE_ICON_COLORS[key] ? { color: LINE_ICON_COLORS[key] } : undefined}
      aria-hidden="true"
    >
      {shape}
    </svg>
  );
}
