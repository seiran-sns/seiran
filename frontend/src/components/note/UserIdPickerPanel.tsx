import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ActorSuggestion, api } from "../../api/client";
import Avatar from "./Avatar";
import styles from "./UserIdPickerPanel.module.css";

interface UserIdPickerPanelProps {
  onPick: (actor: ActorSuggestion) => void;
}

/**
 * ユーザーIDを検索・選択するピッカー本体（Modal内に描画する）。リスト編集画面の
 * メンバー追加ボックス（`ListDetailPanel`）と同じ `api.actors.search` ＋
 * サジェストリストのUIパターンを、投稿本文への挿入用に流用したもの。
 */
export default function UserIdPickerPanel({ onPick }: UserIdPickerPanelProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<ActorSuggestion[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setResults([]);
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      api.actors
        .search(q, 20, controller.signal)
        .then((rows) => {
          if (!cancelled) setResults(rows);
        })
        .catch(() => {})
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    }, 300);
    return () => {
      cancelled = true;
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [query]);

  return (
    <div className={styles.wrap}>
      <input
        type="text"
        className={styles.search}
        placeholder={t("lists:listsSettingsPage.memberSearchPlaceholder")}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        autoComplete="off"
        autoFocus
      />
      <div className={styles.body}>
        {loading ? (
          <p className={styles.message}>{t("common:loading")}</p>
        ) : query.trim() && results.length === 0 ? (
          <p className={styles.message}>{t("home:postComposer.insertUserId.noResults")}</p>
        ) : (
          <ul className={styles.list}>
            {results.map((s) => (
              <li key={s.actor_id}>
                <button
                  type="button"
                  className={styles.item}
                  onClick={() => onPick(s)}
                >
                  <Avatar url={s.avatar_url} name={s.display_name || s.username} size={28} />
                  <span className={styles.name}>
                    {s.display_name || s.username}
                    <span className={styles.handle}>
                      @{s.username}
                      {s.domain ? `@${s.domain}` : ""}
                    </span>
                  </span>
                  <span className={styles.type}>{s.actor_type}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
