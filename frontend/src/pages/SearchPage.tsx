import { FormEvent, useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { api, Note, getErrorMessage } from "../api/client";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import NotificationsPanel from "../components/right/NotificationsPanel";
import { useRightPane } from "../contexts/RightPaneContext";
import panel from "../components/common/Panel.module.css";
import rp from "../components/right/RightPanels.module.css";
import styles from "./HomePage.module.css";

export default function SearchPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const initialQ = searchParams.get("q") ?? "";
  const { timelineTab, setTimelineTab } = useRightPane();

  const [input, setInput] = useState(initialQ);
  const [notes, setNotes] = useState<Note[]>([]);
  const [sessionId, setSessionId] = useState<string | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState("");
  const [searched, setSearched] = useState(false);

  // URL の q が変わったら（右ペインからの遷移など）検索を実行。
  useEffect(() => {
    const q = searchParams.get("q") ?? "";
    setInput(q);
    if (!q.trim()) {
      setNotes([]);
      setSearched(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError("");
    setSessionId(undefined);
    api.notes
      .search({ q: q.trim(), limit: 30 })
      .then((res) => {
        if (cancelled) return;
        setNotes(res.notes);
        setSessionId(res.session_id);
        setSearched(true);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [searchParams]);

  function submit(e: FormEvent) {
    e.preventDefault();
    const q = input.trim();
    if (q) setSearchParams({ q });
  }

  async function loadMore() {
    const q = (searchParams.get("q") ?? "").trim();
    if (!q || loadingMore) return;
    setLoadingMore(true);
    try {
      const res = await api.notes.search({ q, limit: 30, session_id: sessionId });
      setNotes((prev) => [...prev, ...res.notes]);
      setSessionId(res.session_id);
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setLoadingMore(false);
    }
  }

  const center = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>検索</span>
      </header>

      <form className={rp.searchForm} onSubmit={submit}>
        <input
          className={rp.searchInput}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="キーワードを検索（ローカル + Bluesky）"
        />
        <button type="submit" className={rp.searchBtn}>検索</button>
      </form>

      {error && <p className={panel.message}>{error}</p>}

      <NoteList
        notes={notes}
        loading={loading}
        emptyMessage={searched ? "一致する投稿が見つかりませんでした。" : "キーワードを入力して検索してください。"}
      />

      {searched && notes.length > 0 && (
        <div className={styles.feedTabs} style={{ position: "static" }}>
          <button
            className={styles.feedTab}
            onClick={loadMore}
            disabled={loadingMore}
            style={{ cursor: loadingMore ? "default" : "pointer" }}
          >
            {loadingMore ? "読み込み中..." : "さらに読み込む"}
          </button>
        </div>
      )}
    </>
  );

  const right = (
    <>
      <Tabs tabs={["トレンド＆検索", "クイック通知"]} active={timelineTab} onChange={setTimelineTab} />
      {timelineTab === 0 ? (
        <div className={panel.placeholder}>
          <span className={panel.placeholderIcon}>📈</span>
          トレンド集計は準備中です。
        </div>
      ) : (
        <NotificationsPanel />
      )}
    </>
  );

  return <AppShell center={center} right={right} />;
}
