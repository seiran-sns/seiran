import { FormEvent, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import panel from "../common/Panel.module.css";
import styles from "./RightPanels.module.css";

/** ホーム右ペイン タブ1: トレンド＆検索（Doc5 §2.1）。 */
export default function TrendsSearchPanel() {
  const { t } = useTranslation();
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
          placeholder={t("search:trendsSearchPanel.placeholder")}
        />
        <button type="submit" className={styles.searchBtn}>{t("common:search")}</button>
      </form>

      <div className={panel.rightHeader}>{t("search:trendsSearchPanel.trendsHeader")}</div>
      <div className={panel.placeholder}>
        <span className={panel.placeholderIcon}>📈</span>
        {t("search:trendsSearchPanel.comingSoon")}
        <br />
        {t("search:trendsSearchPanel.comingSoonDetail")}
      </div>
    </div>
  );
}
