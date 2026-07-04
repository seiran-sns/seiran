import { useState } from "react";
import styles from "./Avatar.module.css";

interface AvatarProps {
  url?: string;
  name: string;
  /** ピクセルサイズ（正方形）。デフォルト 40。 */
  size?: number;
}

/** ユーザーアイコン。画像があれば表示、無ければ頭文字のプレースホルダ（issue #21）。 */
export default function Avatar({ url, name, size = 40 }: AvatarProps) {
  const [failed, setFailed] = useState(false);
  const initial = (name || "?")[0]?.toUpperCase() ?? "?";
  const dim = { width: size, height: size, fontSize: size * 0.45 };

  if (url && !failed) {
    return (
      <img
        src={url}
        alt=""
        className={styles.img}
        style={{ width: size, height: size }}
        loading="lazy"
        onError={() => setFailed(true)}
      />
    );
  }
  return (
    <span className={styles.fallback} style={dim} aria-hidden>
      {initial}
    </span>
  );
}
