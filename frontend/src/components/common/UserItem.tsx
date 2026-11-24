import Avatar from "../note/Avatar";
import styles from "./UserItem.module.css";
import DisplayName from "./DisplayName";

export interface UserItemProps {
  avatarUrl?: string;
  username: string;
  displayName?: string;
  domain?: string;
  /** 表示名中の `:shortcode:` → 画像URL（アクターの `emojis`）。 */
  emojis?: Record<string, string>;
  /** アバターのサイズ（px）。デフォルト 24。 */
  avatarSize?: number;
  /** 追加のCSSクラス名 */
  className?: string;
}

/** ユーザーのアバター・表示名を表示する共通コンポーネント。 */
export default function UserItem({
  avatarUrl,
  username,
  displayName,
  emojis,
  avatarSize = 24,
  className,
}: UserItemProps) {
  const name = displayName || username;

  return (
    <div className={`${styles.userItem} ${className || ""}`}>
      <Avatar url={avatarUrl} name={name} size={avatarSize} />
      <span className={styles.name}><DisplayName name={displayName} fallback={username} emojis={emojis} /></span>
    </div>
  );
}
