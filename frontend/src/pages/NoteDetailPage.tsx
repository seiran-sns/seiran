import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, Note, getErrorMessage } from "../api/client";
import RemoteBanner from "../components/common/RemoteBanner";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import NoteCard from "../components/note/NoteCard";
import AuthorPanel from "../components/right/AuthorPanel";
import ReactionListPanel from "../components/right/ReactionListPanel";
import ReplyThreadPanel from "../components/right/ReplyThreadPanel";
import RepostListPanel from "../components/right/RepostListPanel";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import { useRightPane } from "../contexts/RightPaneContext";
import panel from "../components/common/Panel.module.css";
import styles from "./NoteDetailPage.module.css";

export default function NoteDetailPage() {
  const { t } = useTranslation();
  const { id } = useParams<{ id: string }>();
  const goBack = useGoBack();
  const { noteDetailTab, setNoteDetailTab } = useRightPane();

  const [note, setNote] = useState<Note | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  // 前後の投稿はボタン押下で初めて読み込む（遅延ロード）。最大5件ずつ、読み込みボタンで継続取得する（#226）。
  // before: 対象ポストに近い順（DESC）。after: 対象ポストに近い順（ASC）。
  const [before, setBefore] = useState<Note[]>([]);
  const [after, setAfter] = useState<Note[]>([]);
  const [ctxRequested, setCtxRequested] = useState(false);
  const [ctxLoading, setCtxLoading] = useState(false);
  const [ctxLoaded, setCtxLoaded] = useState(false);
  const [hasMoreOlder, setHasMoreOlder] = useState(false);
  const [hasMoreNewer, setHasMoreNewer] = useState(false);
  const [loadingMoreOlder, setLoadingMoreOlder] = useState(false);
  const [loadingMoreNewer, setLoadingMoreNewer] = useState(false);

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
    setHasMoreOlder(false);
    setHasMoreNewer(false);
    api.notes
      .get(id)
      .then((n) => !cancelled && setNote(n))
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [id]);

  const CONTEXT_PAGE_SIZE = 5;

  function loadContext() {
    if (!id || ctxRequested) return;
    setCtxRequested(true);
    setCtxLoading(true);
    api.notes
      .context(id, { beforeLimit: CONTEXT_PAGE_SIZE, afterLimit: CONTEXT_PAGE_SIZE })
      .then((ctx) => {
        setBefore(ctx.before);
        setAfter(ctx.after);
        setHasMoreOlder(ctx.before.length === CONTEXT_PAGE_SIZE);
        setHasMoreNewer(ctx.after.length === CONTEXT_PAGE_SIZE);
        setCtxLoaded(true);
      })
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setCtxLoading(false));
  }

  function loadMoreOlder() {
    if (!id || loadingMoreOlder) return;
    const anchor = before[before.length - 1]?.id;
    if (!anchor) return;
    setLoadingMoreOlder(true);
    api.notes
      .context(id, { beforeId: anchor, beforeLimit: CONTEXT_PAGE_SIZE, afterLimit: 0 })
      .then((ctx) => {
        setBefore((prev) => [...prev, ...ctx.before]);
        setHasMoreOlder(ctx.before.length === CONTEXT_PAGE_SIZE);
      })
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoadingMoreOlder(false));
  }

  function loadMoreNewer() {
    if (!id || loadingMoreNewer) return;
    const anchor = after[after.length - 1]?.id;
    if (!anchor) return;
    setLoadingMoreNewer(true);
    api.notes
      .context(id, { afterId: anchor, afterLimit: CONTEXT_PAGE_SIZE, beforeLimit: 0 })
      .then((ctx) => {
        setAfter((prev) => [...prev, ...ctx.after]);
        setHasMoreNewer(ctx.after.length === CONTEXT_PAGE_SIZE);
      })
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoadingMoreNewer(false));
  }

  // リポスト詳細（#45）: リアクションタブはリポスト元のリアクションを表示する。
  const display = note?.renote ?? note;

  // 「投稿主の前後」ブロック（ボタン → 読み込み → 一覧）。中央・右ペインで共用。
  // 表示順は上から: [もっと新しいポストを読み込む] 新しいポスト(最大5件、対象に近い順で下寄り)
  // → 対象ポスト自身(拡大文字表示NoteCard) → 古いポスト(最大5件、対象に近い順で上寄り)
  // → [もっと古いポストを読み込む]。
  // `includeTarget`: 対象ポスト自身をリスト中央に埋め込むか。中央ペインの狭幅表示では
  // 直上に同じ大型NoteCardが既に表示されているため二重表示を避け、右ペインの
  // 「投稿主の前後」タブでのみ埋め込む。
  function renderContext(includeTarget = false) {
    if (!ctxRequested) {
      return (
        <div className={styles.ctxTrigger}>
          <button className={styles.ctxButton} onClick={loadContext}>
            {t("home:noteDetailPage.showContextButton")}
          </button>
        </div>
      );
    }
    if (ctxLoading) return <p className={panel.message}>{t("common:loading")}</p>;
    const targetCard = includeTarget && note ? <NoteCard note={note} large linkToDetail={false} /> : null;
    if (ctxLoaded && before.length === 0 && after.length === 0) {
      return (
        <div>
          {targetCard}
          <p className={panel.message}>{t("home:noteDetailPage.noContext")}</p>
        </div>
      );
    }
    const newerDesc = [...after].reverse();
    return (
      <div>
        {hasMoreNewer && (
          <div className={styles.ctxTrigger}>
            <button className={styles.ctxButton} onClick={loadMoreNewer} disabled={loadingMoreNewer}>
              {loadingMoreNewer ? t("common:loading") : t("home:noteDetailPage.loadNewerButton")}
            </button>
          </div>
        )}
        {newerDesc.map((n) => (
          <NoteCard key={n.id} note={n} />
        ))}
        {targetCard}
        {before.map((n) => (
          <NoteCard key={n.id} note={n} />
        ))}
        {hasMoreOlder && (
          <div className={styles.ctxTrigger}>
            <button className={styles.ctxButton} onClick={loadMoreOlder} disabled={loadingMoreOlder}>
              {loadingMoreOlder ? t("common:loading") : t("home:noteDetailPage.loadOlderButton")}
            </button>
          </div>
        )}
      </div>
    );
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("home:noteDetailPage.title")}</span>
      </header>

      {loading && <p className={panel.message}>{t("common:loading")}</p>}
      {error && <p className={panel.message}>{error}</p>}

      {/* リポスト詳細（#45）: 表示すべき実体（リポスト元があればそちら）でリモート判定する。 */}
      {display && display.user.actorType !== "local" && display.remoteUrl && (
        <RemoteBanner message={t("common:remoteBanner.note")} url={display.remoteUrl} />
      )}

      {note && (
        <>
          {/* 主役ポストはタイムラインと同じ NoteCard を大型表示で共用する（#43）。リポスト表示は NoteCard 内部で処理（#45）。 */}
          <NoteCard note={note} large linkToDetail={false} />

          {/* 投稿主の前後の投稿（右ペインが隠れる幅でのみ中央に表示。ボタン起動）。 */}
          <section className={styles.narrowContext}>
            <div className={styles.contextLabel}>{t("home:noteDetailPage.contextLabel")}</div>
            {renderContext()}
          </section>
        </>
      )}
    </>
  );

  const right = (
    <>
      <Tabs
        tabs={[
          t("home:noteDetailPage.authorTab"),
          t("home:noteDetailPage.repliesTab"),
          t("home:noteDetailPage.contextTab"),
          t("home:noteDetailPage.reactionsTab"),
          t("home:noteDetailPage.repostsTab"),
        ]}
        active={noteDetailTab}
        onChange={setNoteDetailTab}
      />
      {noteDetailTab === 0 && display && <AuthorPanel note={display} />}
      {noteDetailTab === 1 && display && <ReplyThreadPanel note={display} />}
      {noteDetailTab === 2 && renderContext(true)}
      {noteDetailTab === 3 && display && <ReactionListPanel note={display} />}
      {noteDetailTab === 4 && display && <RepostListPanel note={display} />}
    </>
  );

  return <AppShell center={center} right={right} />;
}
