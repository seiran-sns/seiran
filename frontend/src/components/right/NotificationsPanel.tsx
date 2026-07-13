import { useEffect } from "react";
import { Notif, useStreamingContext } from "../../contexts/StreamingContext";
import panel from "../common/Panel.module.css";
import styles from "./NotificationsPanel.module.css";

/** 通知1件を人間可読な文言に整形する。`iconUrl` があれば絵文字は画像（カスタム絵文字）。 */
function describe(n: Notif): { icon: string; iconUrl?: string; text: string } {
  const actor = n.body.actor;
  const who = actor?.displayName || actor?.username || "だれか";
  const handle = actor?.username && actor?.domain ? `@${actor.username}@${actor.domain}` : "";
  const label = handle ? `${who}（${handle}）` : who;
  switch (n.kind) {
    case "reaction":
      return { icon: n.body.emoji || "⭐", iconUrl: n.body.emojiUrl, text: `${label} がリアクションしました` };
    case "follow":
      return { icon: "➕", text: `${label} にフォローされました` };
    case "followAccepted":
      return { icon: "🤝", text: `${label} がフォローを承認しました` };
    default:
      return { icon: "🔔", text: `${label} から通知` };
  }
}

/** ホーム右ペイン タブ2: クイック通知（Doc5 §2.1）。 */
export default function NotificationsPanel() {
  const { notifications, markRead } = useStreamingContext();

  // このパネルを開いている間は既読扱いにする。
  useEffect(() => {
    markRead();
  }, [notifications.length, markRead]);

  if (notifications.length === 0) {
    return (
      <div className={panel.placeholder}>
        <span className={panel.placeholderIcon}>🔔</span>
        新しい通知はありません。
        <br />
        リプライ・リアクション・フォローがここにリアルタイム表示されます。
      </div>
    );
  }

  return (
    <ul className={styles.list}>
      {notifications.map((n) => {
        const { icon, iconUrl, text } = describe(n);
        return (
          <li key={n.id} className={styles.item}>
            {iconUrl ? (
              <img className={styles.iconImg} src={iconUrl} alt={icon} title={icon} loading="lazy" />
            ) : (
              <span className={styles.icon}>{icon}</span>
            )}
            <span className={styles.text}>{text}</span>
          </li>
        );
      })}
    </ul>
  );
}
