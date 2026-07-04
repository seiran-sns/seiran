import { useEffect, useState } from "react";
import { api, Note } from "../api/client";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import PostComposer from "../components/note/PostComposer";
import NotificationsPanel from "../components/right/NotificationsPanel";
import TrendsSearchPanel from "../components/right/TrendsSearchPanel";
import { useRightPane } from "../contexts/RightPaneContext";
import panel from "../components/common/Panel.module.css";
import styles from "./HomePage.module.css";

type Feed = "local" | "home";

export default function HomePage() {
  const [feed, setFeed] = useState<Feed>("home");
  const [notes, setNotes] = useState<Note[]>([]);
  const [loading, setLoading] = useState(true);
  const { timelineTab, setTimelineTab } = useRightPane();

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const fetch = feed === "home" ? api.notes.homeTimeline({ limit: 30 }) : api.notes.localTimeline({ limit: 30 });
    fetch
      .then((n) => !cancelled && setNotes(n))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [feed]);

  function prepend(note: Note) {
    setNotes((prev) => [note, ...prev]);
  }

  const center = (
    <>
      <header className={panel.header}>
        <span className={panel.title}>ホーム</span>
      </header>

      <div className={styles.composerWrap}>
        <PostComposer onPosted={prepend} />
      </div>

      <div className={styles.feedTabs}>
        <button
          className={`${styles.feedTab} ${feed === "home" ? styles.feedTabActive : ""}`}
          onClick={() => setFeed("home")}
        >
          ホーム
        </button>
        <button
          className={`${styles.feedTab} ${feed === "local" ? styles.feedTabActive : ""}`}
          onClick={() => setFeed("local")}
        >
          ローカル
        </button>
      </div>

      <NoteList
        notes={notes}
        loading={loading}
        emptyMessage={
          feed === "home"
            ? "フォロー中のユーザーの投稿がここに表示されます。"
            : "まだ投稿がありません。最初の投稿をしてみましょう！"
        }
      />
    </>
  );

  const right = (
    <>
      <Tabs
        tabs={["トレンド＆検索", "クイック通知"]}
        active={timelineTab}
        onChange={setTimelineTab}
      />
      {timelineTab === 0 ? <TrendsSearchPanel /> : <NotificationsPanel />}
    </>
  );

  return <AppShell center={center} right={right} onPosted={prepend} />;
}
