import { useEffect } from "react";
import { Notif, useStreamingContext } from "../../contexts/StreamingContext";
import panel from "../common/Panel.module.css";
import styles from "./NotificationsPanel.module.css";

/** 通知1件を人間可読な文言に整形する。 */
function describe(n: Notif): { icon: string; text: string } {
  const actor = n.body.actor;
  const who = actor?.displayName || actor?.username || "だれか";
  const handle = actor?.username && actor?.domain ? `@${actor.username}@${actor.domain}` : "";
  const label = handle ? `${who}（${handle}）` : who;
  switch (n.kind) {
    case "reaction":
      return { icon: n.body.emoji || "⭐", text: `${label} がリアクションしました` };
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
        const { icon, text } = describe(n);
        return (
          <li key={n.id} className={styles.item}>
            <span className={styles.icon}>{icon}</span>
            <span className={styles.text}>{text}</span>
          </li>
        );
      })}
    </ul>
  );
}
