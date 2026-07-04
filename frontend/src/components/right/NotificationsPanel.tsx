import panel from "../common/Panel.module.css";

/** ホーム右ペイン タブ2: クイック通知（Doc5 §2.1）。 */
export default function NotificationsPanel() {
  return (
    <div className={panel.placeholder}>
      <span className={panel.placeholderIcon}>🔔</span>
      自分へのリプライ・リアクションの簡易ストリームは準備中です。
      <br />
      通知エンドポイントの実装後に有効化されます。
    </div>
  );
}
