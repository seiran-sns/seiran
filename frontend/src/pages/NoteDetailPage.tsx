import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { api, Note, getErrorMessage } from "../api/client";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import NoteCard from "../components/note/NoteCard";
import ReactionChips from "../components/note/ReactionChips";
import { useRightPane } from "../contexts/RightPaneContext";
import panel from "../components/common/Panel.module.css";
import styles from "./NoteDetailPage.module.css";

export default function NoteDetailPage() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const { noteDetailTab, setNoteDetailTab } = useRightPane();

  const [note, setNote] = useState<Note | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  // 前後の投稿はボタン押下で初めて読み込む（遅延ロード）。
  const [before, setBefore] = useState<Note[]>([]);
  const [after, setAfter] = useState<Note[]>([]);
  const [ctxRequested, setCtxRequested] = useState(false);
  const [ctxLoading, setCtxLoading] = useState(false);
  const [ctxLoaded, setCtxLoaded] = useState(false);

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setLoading(true);
    setError("");
    // ノートが切り替わったら前後投稿の状態をリセット（再度ボタン押下が必要）。
    setBefore([]);
    setAfter([]);
    setCtxRequested(false);
    setCtxLoading(false);
    setCtxLoaded(false);
    api.notes
      .get(id)
      .then((n) => !cancelled && setNote(n))
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [id]);

  function loadContext() {
    if (!id || ctxRequested) return;
    setCtxRequested(true);
    setCtxLoading(true);
    api.notes
      .context(id)
      .then((ctx) => {
        setBefore(ctx.before);
        setAfter(ctx.after);
        setCtxLoaded(true);
      })
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setCtxLoading(false));
  }

  const contextList = [...before].reverse().concat(after);

  // 「投稿主の前後」ブロック（ボタン → 読み込み → 一覧）。中央・右ペインで共用。
  function renderContext() {
    if (!ctxRequested) {
      return (
        <div className={styles.ctxTrigger}>
          <button className={styles.ctxButton} onClick={loadContext}>
            前後の投稿を表示
          </button>
        </div>
      );
    }
    if (ctxLoading) return <p className={panel.message}>読み込み中...</p>;
    if (ctxLoaded && contextList.length === 0) {
      return <p className={panel.message}>前後の投稿はありません。</p>;
    }
    return (
      <div>
        {contextList.map((n) => (
          <NoteCard key={n.id} note={n} />
        ))}
      </div>
    );
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={() => navigate(-1)}>
          ← 戻る
        </button>
        <span className={panel.title}>ポスト</span>
      </header>

      {loading && <p className={panel.message}>読み込み中...</p>}
      {error && <p className={panel.message}>{error}</p>}

      {note && (
        <>
          {/* 主役ポストはタイムラインと同じ NoteCard を大型表示で共用する（#43）。 */}
          <NoteCard note={note} large linkToDetail={false} />

          {/* 投稿主の前後の投稿（右ペインが隠れる幅でのみ中央に表示。ボタン起動）。 */}
          <section className={styles.narrowContext}>
            <div className={styles.contextLabel}>投稿主の前後の投稿</div>
            {renderContext()}
          </section>

          {/* 直系リプライ・引用（専用 API 未実装のためプレースホルダ） */}
          <div className={panel.placeholder}>
            <span className={panel.placeholderIcon}>💬</span>
            直系リプライ・引用ポストのツリー表示は準備中です。
          </div>
        </>
      )}
    </>
  );

  const right = (
    <>
      <Tabs
        tabs={["投稿主の前後", "リアクション"]}
        active={noteDetailTab}
        onChange={setNoteDetailTab}
      />
      {noteDetailTab === 0 ? (
        renderContext()
      ) : note && note.reactions && note.reactions.length > 0 ? (
        <div style={{ padding: "12px 16px" }}>
          <ReactionChips reactions={note.reactions} />
        </div>
      ) : (
        <div className={panel.placeholder}>
          <span className={panel.placeholderIcon}>😀</span>
          このポストにはまだリアクションがありません。
        </div>
      )}
    </>
  );

  return <AppShell center={center} right={right} />;
}
