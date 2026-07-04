import { FormEvent, useState } from "react";
import { useNavigate } from "react-router-dom";
import panel from "../common/Panel.module.css";
import styles from "./RightPanels.module.css";

/** ホーム右ペイン タブ1: トレンド＆検索（Doc5 §2.1）。 */
export default function TrendsSearchPanel() {
  const [q, setQ] = useState("");
  const navigate = useNavigate();

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    const query = q.trim();
    if (query) navigate(`/search?q=${encodeURIComponent(query)}`);
  }

  return (
    <div>
      <form className={styles.searchForm} onSubmit={onSubmit}>
        <input
          className={styles.searchInput}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="キーワードを検索"
        />
        <button type="submit" className={styles.searchBtn}>検索</button>
      </form>

      <div className={panel.rightHeader}>トレンド</div>
      <div className={panel.placeholder}>
        <span className={panel.placeholderIcon}>📈</span>
        二大宇宙（AP / ATP）の集計によるリアルタイムトレンドは準備中です。
        <br />
        集計エンジンの実装後に有効化されます。
      </div>
    </div>
  );
}
