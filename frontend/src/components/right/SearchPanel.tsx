import { FormEvent, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import styles from "./RightPanels.module.css";

/** 右ペインの【検索】タブ: 検索窓だけを置き、送信で検索画面へ移る。 */
export default function SearchPanel() {
  const { t } = useTranslation();
  const [q, setQ] = useState("");
  const navigate = useNavigate();

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    const query = q.trim();
    if (query) navigate(`/search?q=${encodeURIComponent(query)}`);
  }

  return (
    <form className={styles.searchForm} onSubmit={onSubmit}>
      <input
        className={styles.searchInput}
        value={q}
        onChange={(e) => setQ(e.target.value)}
        placeholder={t("search:searchPanel.placeholder")}
      />
      <button type="submit" className={styles.searchBtn}>{t("common:search")}</button>
    </form>
  );
}
