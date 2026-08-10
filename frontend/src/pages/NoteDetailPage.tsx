import { useEffect, useRef, useState } from "react";
import { useParams, useSearchParams } from "react-router-dom";
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

const TAB_COUNT = 5;

export default function NoteDetailPage() {
  const { t } = useTranslation();
  const { id } = useParams<{ id: string }>();
  const goBack = useGoBack();
  const { noteDetailTab, setNoteDetailTab, noteContextScroll, setNoteContextScroll } = useRightPane();
  const [searchParams, setSearchParams] = useSearchParams();

  const [note, setNote] = useState<Note | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  // 前後の投稿は対象ポストの取得と同時に自動で読み込む（#226、ボタン操作不要）。
  // 最大5件ずつ、続きは読み込みボタンで継続取得する。
  // before: 対象ポストに近い順（DESC）。after: 対象ポストに近い順（ASC）。
  const CONTEXT_PAGE_SIZE = 5;
  const [before, setBefore] = useState<Note[]>([]);
  const [after, setAfter] = useState<Note[]>([]);
  const [ctxLoading, setCtxLoading] = useState(true);
  const [ctxLoaded, setCtxLoaded] = useState(false);
  const [hasMoreOlder, setHasMoreOlder] = useState(false);
  const [hasMoreNewer, setHasMoreNewer] = useState(false);
  const [loadingMoreOlder, setLoadingMoreOlder] = useState(false);
  const [loadingMoreNewer, setLoadingMoreNewer] = useState(false);
  // 「前後のポスト」タブを開いた際、読み込み完了後（かつ復元すべきスクロール位置が
  // 無い場合）に対象ポスト自身へスクロールするための参照。
  const targetCardRef = useRef<HTMLDivElement>(null);
  // 右ペイン全体（AppShellの独立スクロール領域である<aside>）を closest() で辿るための参照。
  const rightPaneRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setLoading(true);
    setError("");
    api.notes
      .get(id)
      .then((n) => !cancelled && setNote(n))
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [id]);

  // タブ選択状態をURLの?tab=と同期する（#226）。マウント時（＝ノート切り替え時）に
  // URLへ既にタブ番号が入っていればそれを復元し（ブラウザリロード後もタブ選択を維持するため）、
  // 無ければ RightPaneContext が持つ現在値（他ポスト間でのセッション内タブ記憶）をそのまま使う。
  useEffect(() => {
    const tabParam = searchParams.get("tab");
    if (tabParam === null) return;
    const n = Number(tabParam);
    if (Number.isInteger(n) && n >= 0 && n < TAB_COUNT) setNoteDetailTab(n);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  // タブが変わるたびURLへ反映する（履歴を汚さないよう置き換え）。
  useEffect(() => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.set("tab", String(noteDetailTab));
        return next;
      },
      { replace: true },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [noteDetailTab]);

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setBefore([]);
    setAfter([]);
    setCtxLoading(true);
    setCtxLoaded(false);
    setHasMoreOlder(false);
    setHasMoreNewer(false);
    api.notes
      .context(id, { beforeLimit: CONTEXT_PAGE_SIZE, afterLimit: CONTEXT_PAGE_SIZE })
      .then((ctx) => {
        if (cancelled) return;
        setBefore(ctx.before);
        setAfter(ctx.after);
        setHasMoreOlder(ctx.before.length === CONTEXT_PAGE_SIZE);
        setHasMoreNewer(ctx.after.length === CONTEXT_PAGE_SIZE);
        setCtxLoaded(true);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setCtxLoading(false));
    return () => {
      cancelled = true;
    };
  }, [id]);

  // 「前後のポスト」タブ（noteDetailTab === 2）を開いていて読み込みが完了したら、
  // このポストについて記憶済みのスクロール位置があればそれを復元し（ブラウザバックで
  // 同じポストへ戻った際の再現用）、無ければ対象ポスト自身の位置までスクロールする。
  useEffect(() => {
    if (noteDetailTab !== 2 || !ctxLoaded || !id) return;
    const asideEl = rightPaneRef.current?.closest("aside");
    const saved = noteContextScroll[id];
    if (asideEl && saved !== undefined) {
      asideEl.scrollTop = saved;
    } else if (targetCardRef.current) {
      targetCardRef.current.scrollIntoView({ block: "center" });
    }
    // noteContextScroll自体を依存に含めると復元直後の保存で無限ループするため、idの変化のみを見る。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [noteDetailTab, ctxLoaded, id]);

  // 「前後のポスト」タブから離れる（タブ切り替え・ポスト切り替え・アンマウント）直前に
  // 右ペインのスクロール位置を保存する（#226）。
  useEffect(() => {
    if (noteDetailTab !== 2 || !id) return;
    const asideEl = rightPaneRef.current?.closest("aside");
    return () => {
      if (asideEl) setNoteContextScroll(id, asideEl.scrollTop);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [noteDetailTab, id]);

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

  // 「前後のポスト」ブロック（自動読み込み → 一覧）。中央・右ペインで共用。
  // 表示順は上から: [もっと新しいポストを読み込む] 新しいポスト(最大5件、対象に近い順で下寄り)
  // → 対象ポスト自身(拡大文字表示NoteCard) → 古いポスト(最大5件、対象に近い順で上寄り)
  // → [もっと古いポストを読み込む]。
  // `includeTarget`: 対象ポスト自身をリスト中央に埋め込むか。中央ペインの狭幅表示では
  // 直上に同じ大型NoteCardが既に表示されているため二重表示を避け、右ペインの
  // 「前後のポスト」タブでのみ埋め込む。
  function renderContext(includeTarget = false) {
    if (ctxLoading) return <p className={panel.message}>{t("common:loading")}</p>;
    const targetCard =
      includeTarget && note ? (
        <div ref={targetCardRef}>
          <NoteCard note={note} large linkToDetail={false} />
        </div>
      ) : null;
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

          {/* 前後のポスト（右ペインが隠れる幅でのみ中央に表示。自動読み込み）。 */}
          <section className={styles.narrowContext}>
            <div className={styles.contextLabel}>{t("home:noteDetailPage.contextLabel")}</div>
            {renderContext()}
          </section>
        </>
      )}
    </>
  );

  const right = (
    // display: contents でレイアウトに影響を与えず、closest("aside") で
    // AppShellの独立スクロール領域（右ペイン本体）を辿るための参照だけを提供する。
    <div ref={rightPaneRef} style={{ display: "contents" }}>
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
        sticky
        top={0}
      />
      {noteDetailTab === 0 && display && <AuthorPanel note={display} />}
      {noteDetailTab === 1 && display && <ReplyThreadPanel note={display} />}
      {noteDetailTab === 2 && renderContext(true)}
      {noteDetailTab === 3 && display && <ReactionListPanel note={display} />}
      {noteDetailTab === 4 && display && <RepostListPanel note={display} />}
    </div>
  );

  return <AppShell center={center} right={right} />;
}
