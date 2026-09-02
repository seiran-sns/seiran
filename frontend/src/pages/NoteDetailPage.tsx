import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation, useNavigate, useParams, useSearchParams } from "react-router-dom";
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
import { useIsNarrowViewport } from "../hooks/useIsNarrowViewport";
import panel from "../components/common/Panel.module.css";
import styles from "./NoteDetailPage.module.css";

const TAB_COUNT = 5;

export default function NoteDetailPage() {
  const { t } = useTranslation();
  const { id } = useParams<{ id: string }>();
  const goBack = useGoBack();
  const {
    noteDetailTab,
    setNoteDetailTab,
    noteContextScroll,
    setNoteContextScroll,
    noteAncestorIds,
    setNoteAncestorIds,
    noteDetailScrollY,
    setNoteDetailScrollY,
  } = useRightPane();
  const [searchParams] = useSearchParams();
  const location = useLocation();
  const navigate = useNavigate();
  // 右ペインが無い狭幅表示かどうか（#56/#61と同じ判定）。返信タブは3ペイン表示のときだけ
  // 中央ペイン下部の常設セクションへ移し、狭幅ではこれまで通り右ペインのタブの1つとする。
  const isNarrow = useIsNarrowViewport();
  const repliesInCenter = !isNarrow;

  const [note, setNote] = useState<Note | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  // `#open_cw` 付きURLでの遷移時、CWを開いた状態で表示する（#229）。
  const [forceOpenCw, setForceOpenCw] = useState(false);

  // 対象ポストが返信だった場合に上へ積み上げていく返信先チェーン（古い順、末尾が直近の親）。
  const [ancestors, setAncestors] = useState<Note[]>([]);
  const [ancestorsReady, setAncestorsReady] = useState(false);

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
  // 本体（主役ポスト）のNoteCardへの参照。初回訪問時、積み上げた返信先チェーンが長く
  // 本体が画面外に押し出されている場合に限り、本体が見える位置までスクロールするために使う。
  const mainCardRef = useRef<HTMLDivElement>(null);
  // 中央ペインのスクロール位置（window.scrollY）を、どのノートIDについて最後に復元したかを
  // 記録する（同じノートに対して二重に復元し直さないため）。
  const scrollRestoredForRef = useRef<string | null>(null);
  // 他画面へ遷移中かどうか。React 18のuseEffect cleanupはunmount時も非同期（paint後）に
  // 実行されるため、遷移でDOMの高さが変わりブラウザが自動でscrollYを0へ補正した際、
  // まだ解除されていない旧scrollリスナーがその0を保存してしまう競合がある（HomePageの
  // navigatingAway/onBeforeNavigateと同じ問題・同じ対策）。リンククリック時点で同期的に
  // trueにして以降の自動保存を止める。
  const navigatingAway = useRef(false);

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

  // `#open_cw` ハッシュ付きでの遷移を検出する（#229）。タブ同期effect（下記）が
  // URLへ書き戻す際にハッシュを保持するため、ここで読んでも消えていない。
  useEffect(() => {
    setForceOpenCw(window.location.hash === "#open_cw");
  }, [id]);

  // タブ選択状態をURLの?tab=と同期する（#226）。マウント時（＝ノート切り替え時）に
  // URLへ既にタブ番号が入っていればそれを復元し（ブラウザリロード後もタブ選択を維持するため）、
  // 無ければ RightPaneContext が持つ現在値（他ポスト間でのセッション内タブ記憶）をそのまま使う。
  useEffect(() => {
    const tabParam = searchParams.get("tab");
    if (tabParam === null) return;
    const n = Number(tabParam);
    // リポストラッパー時は「元投稿者」タブが末尾に増えるため上限を+1しておく（TAB_COUNTの
    // 確定はnote読み込み後のためタイミングに依存せず安全側に倒す）。
    if (Number.isInteger(n) && n >= 0 && n < TAB_COUNT + 1) setNoteDetailTab(n);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  // タブが変わるたびURLへ反映する（履歴を汚さないよう置き換え）。
  // react-router-dom v6のsetSearchParamsはsearch部分のみでナビゲートし
  // location.hashを消してしまうため（#open_cwハッシュ経由のCW展開が壊れる）、
  // navigateでハッシュを保ったままURLを組み立てる。
  useEffect(() => {
    const next = new URLSearchParams(location.search);
    next.set("tab", String(noteDetailTab));
    navigate(`${location.pathname}?${next.toString()}${location.hash}`, { replace: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [noteDetailTab]);

  // 返信タブは3ペイン表示（repliesInCenter）では中央ペインの常設セクションへ移るため
  // 右ペインのタブ選択肢から消える。セッション内で記憶されたタブ選択が返信のままだと
  // 右ペインが空になってしまうため、その場合は投稿者タブへ戻す。
  useEffect(() => {
    if (repliesInCenter && noteDetailTab === 1) setNoteDetailTab(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repliesInCenter]);

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

  // 対象ポストが返信だった場合、返信先ポストのNoteCard（small表示）を本体の上に積み上げる。
  // ブラウザバックで戻った際は、セッション内に記憶済みの遡り済みチェーン（noteAncestorIds）
  // があればそれをそのまま再取得して再現し、無ければ直近の親1件だけを自動で読み込む。
  useEffect(() => {
    if (!note || !id) return;
    let cancelled = false;
    setAncestorsReady(false);
    const base = note.renote ?? note;
    const cachedIds = noteAncestorIds[id];
    if (cachedIds && cachedIds.length > 0) {
      Promise.all(cachedIds.map((aid) => api.notes.get(aid)))
        .then((notes) => !cancelled && setAncestors(notes))
        .catch(() => !cancelled && setAncestors([]))
        .finally(() => !cancelled && setAncestorsReady(true));
    } else if (base.replyId) {
      api.notes
        .get(base.replyId)
        .then((parent) => {
          if (cancelled) return;
          setAncestors([parent]);
          setNoteAncestorIds(id, [parent.id]);
        })
        .catch(() => !cancelled && setAncestors([]))
        .finally(() => !cancelled && setAncestorsReady(true));
    } else {
      setAncestors([]);
      setAncestorsReady(true);
    }
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [note?.id]);

  // 積み上げ済みチェーンの一番上（最も古い）ポストがさらに返信だった場合、その「↩️ 返信」を
  // クリックしてもう1段上の返信先ポストを取得し、先頭に積み足す（#スレッドをどんどん遡る）。
  function climbAncestor(replyId: string) {
    api.notes
      .get(replyId)
      .then((parent) => {
        setAncestors((prev) => {
          if (prev.some((a) => a.id === parent.id)) return prev;
          const next = [parent, ...prev];
          if (id) setNoteAncestorIds(id, next.map((a) => a.id));
          return next;
        });
      })
      .catch((e) => setError(getErrorMessage(e)));
  }

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

  // 中央ペイン（window）のスクロール位置は継続的に保存する。HomePageと同じ理由（React 18
  // StrictModeの疑似アンマウントでcleanupが前倒しに発火する）で、離脱時に一度だけではなく
  // scrollイベントごとに即時保存する。
  useEffect(() => {
    if (!id) return;
    const onScroll = () => {
      if (navigatingAway.current) return;
      setNoteDetailScrollY(id, window.scrollY);
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, [id, setNoteDetailScrollY]);

  // AppShell側でリンククリックの捕捉フェーズ（RouterによるDOM差し替え前）に同期的に呼ばれる
  // （AppShell.tsx参照）。ここで確定した位置を保存し、以降の自動保存（上記onScroll）を止める。
  const saveScrollBeforeNavigate = useCallback(() => {
    if (!id) return;
    navigatingAway.current = true;
    setNoteDetailScrollY(id, window.scrollY);
  }, [id, setNoteDetailScrollY]);

  // 積み上げ済みの返信先チェーンが確定（ancestorsReady）してから一度だけ、記憶済みの
  // スクロール位置を復元する。チェーンの高さが確定する前に復元すると位置がズレるため待つ。
  useEffect(() => {
    if (!id || loading || !ancestorsReady) return;
    if (scrollRestoredForRef.current === id) return;
    scrollRestoredForRef.current = id;
    const saved = noteDetailScrollY[id];
    if (saved) {
      requestAnimationFrame(() => requestAnimationFrame(() => window.scrollTo(0, saved)));
    } else {
      // 初回訪問（記憶が無い）: 積み上げた返信先チェーンが長く本体ポストが画面外に
      // 押し出されている場合だけ、ちょうど本体が見える位置までスクロールする。
      // block: "nearest" は既に画面内に収まっていれば何もしないため、短い場合は
      // これまで通り先頭（ヘッダー含む）が見えたままになる。
      requestAnimationFrame(() =>
        requestAnimationFrame(() => mainCardRef.current?.scrollIntoView({ block: "nearest" })),
      );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, loading, ancestorsReady]);

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

  // リポスト詳細（#45）: 返信・前後の投稿・リアクション・リポスト一覧タブはリポスト元の実体を表示する。
  const display = note?.renote ?? note;
  // リポストという行為自体はリポストした人(note自身)の自己表現であるため、投稿者欄・
  // リモート判定・「元投稿者」タブの出し分けはnote自身（B）を基準にする（display=Aとは区別）。
  const hasRenote = Boolean(note?.renote);

  // 右ペインのタブ一覧（意味の固定されたID付き）。返信タブは3ペイン表示では中央ペインの
  // 常設セクションへ移るため、repliesInCenter時はここから除く。既存タブのIDは note を跨いで
  // セッション内保持される noteDetailTab の値と一致させる必要があるため、配列内位置ではなく
  // 固定IDで管理する。
  const allTabs: { id: number; label: string }[] = [
    { id: 0, label: t("home:noteDetailPage.authorTab") },
    { id: 1, label: t("home:noteDetailPage.repliesTab") },
    { id: 2, label: t("home:noteDetailPage.contextTab") },
    { id: 3, label: t("home:noteDetailPage.reactionsTab") },
    { id: 4, label: t("home:noteDetailPage.repostsTab") },
    ...(hasRenote ? [{ id: 5, label: t("home:noteDetailPage.originalAuthorTab") }] : []),
  ];
  const visibleTabs = repliesInCenter ? allTabs.filter((tab) => tab.id !== 1) : allTabs;
  const activeTabIndex = Math.max(
    0,
    visibleTabs.findIndex((tab) => tab.id === noteDetailTab),
  );

  // 「前後のポスト」ブロック（自動読み込み → 一覧、右ペインの「前後のポスト」タブでのみ使う）。
  // 表示順は上から: [もっと新しいポストを読み込む] 新しいポスト(最大5件、対象に近い順で下寄り)
  // → 対象ポスト自身(拡大文字表示NoteCard) → 古いポスト(最大5件、対象に近い順で上寄り)
  // → [もっと古いポストを読み込む]。
  function renderContext() {
    if (ctxLoading) return <p className={panel.message}>{t("common:loading")}</p>;
    const targetCard = note ? (
      <div ref={targetCardRef}>
        <NoteCard note={note} large linkToDetail={false} forceOpenCw={forceOpenCw} />
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

      {/* リポストという行為自体の主体(note自身、リポストラッパーならB)でリモート判定する。 */}
      {note && note.user.actorType !== "local" && note.remoteUrl && (
        <RemoteBanner
          message={t("common:remoteBanner.note")}
          url={note.remoteUrl}
          protocol={note.user.actorType === "bsky" ? "bsky" : "fedi"}
        />
      )}

      {/* 対象ポストが返信だった場合の返信先チェーン（古い順、直近の親が本体の直上）。
          一番上のカードだけ「↩️ 返信」クリックでさらに1段遡れる（#climbAncestor）。 */}
      {ancestors.map((a, i) => (
        <NoteCard
          key={a.id}
          note={a}
          small
          onReplyIndicatorClick={i === 0 ? climbAncestor : undefined}
        />
      ))}

      {note && (
        // 主役ポストはタイムラインと同じ NoteCard を大型表示で共用する（#43）。リポスト表示は NoteCard 内部で処理（#45）。
        <div ref={mainCardRef}>
          <NoteCard note={note} large linkToDetail={false} forceOpenCw={forceOpenCw} />
        </div>
      )}

      {/* 返信タブ（3ペイン表示時のみ）: プロフィール画面のピン留めと同様、右ペインのタブから
          外し中央ペイン下部の常設セクションとして表示する。 */}
      {repliesInCenter && display && (
        <>
          <div className={panel.rightHeader}>{t("home:noteDetailPage.repliesTab")}</div>
          <ReplyThreadPanel note={display} />
        </>
      )}
    </>
  );

  const right = (
    // closest("aside")でAppShellの右ペイン本体を辿るための参照を提供する（#226）。
    // 以前はdisplay:contentsでレイアウトへの影響を避けていたが、通常のdiv（block要素、
    // 子はTabsと切り替え表示パネル1つずつなので縦積みの見た目は変わらない）に変更しても
    // 支障が無いため単純化した。狭幅でposition:stickyが効かなくなる不具合の実体は
    // display:contentsではなくAppShell.module.cssの.rightのoverflow-y残留だった
    // （修正はAppShell.module.css側、#241）。
    <div ref={rightPaneRef}>
      <Tabs
        tabs={visibleTabs.map((tab) => tab.label)}
        active={activeTabIndex}
        onChange={(i) => setNoteDetailTab(visibleTabs[i].id)}
        sticky
        top={0}
      />
      {noteDetailTab === 0 && note && <AuthorPanel note={note} />}
      {noteDetailTab === 1 && display && !repliesInCenter && <ReplyThreadPanel note={display} />}
      {noteDetailTab === 2 && renderContext()}
      {noteDetailTab === 3 && display && <ReactionListPanel note={display} />}
      {noteDetailTab === 4 && display && <RepostListPanel note={display} />}
      {noteDetailTab === 5 && hasRenote && display && <AuthorPanel note={display} />}
    </div>
  );

  return <AppShell center={center} right={right} onBeforeNavigate={saveScrollBeforeNavigate} />;
}
