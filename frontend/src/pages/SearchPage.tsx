import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, ActorSuggestion, Note, PostSearchConditions, getErrorMessage } from "../api/client";
import DisplayName from "../components/common/DisplayName";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import Avatar from "../components/note/Avatar";
import NoteList from "../components/note/NoteList";
import NotificationsPanel from "../components/right/NotificationsPanel";
import SearchConditionsField, {
  hasConditions,
  SearchConditions,
  SearchConditionUser,
} from "../components/search/SearchConditionsField";
import { useRightPane } from "../contexts/RightPaneContext";
import { useInfiniteScrollSentinel } from "../hooks/useInfiniteScrollSentinel";
import { useIsMobileViewport } from "../hooks/useIsNarrowViewport";
import { profilePath } from "../lib/format";
import panel from "../components/common/Panel.module.css";
import rp from "../components/right/RightPanels.module.css";
import listStyles from "../components/right/FollowListPanel.module.css";
import pageStyles from "./SearchPage.module.css";

const PAGE_SIZE = 30;
type SearchTab = "posts" | "users";

/** `YYYY-MM-DD`（閲覧者のタイムゾーンの日付）のその日の始まりを RFC 3339 にする。
 * `dayOffset` 日ずらす（「この日以前」はその日を含めるため翌日の始まりを上限にする）。 */
function dayStartIso(date: string, dayOffset = 0): string | undefined {
  const [y, m, d] = date.split("-").map(Number);
  if (!y || !m || !d) return undefined;
  return new Date(y, m - 1, d + dayOffset).toISOString();
}

/** URL のクエリから条件を読む（ユーザーはハンドルだけ。表示名・ID は別に引き直す）。 */
function conditionsFromParams(params: URLSearchParams): SearchConditions & { userHandle?: string } {
  return {
    since: params.get("since") || undefined,
    until: params.get("until") || undefined,
    userHandle: params.get("user") || undefined,
    server: params.get("server") || undefined,
    tag: params.get("tag") || undefined,
  };
}

/** ポストの検索1回分（「検索」を押した時点の検索語と条件）。`seq` は同じ内容でも押すたびに増やす。 */
interface PostRequest {
  seq: number;
  q: string;
  conditions: SearchConditions;
}

/** ユーザーの検索1回分（「検索」を押した時点の検索語）。`seq` は同じ内容でも押すたびに増やす。 */
interface UserRequest {
  seq: number;
  q: string;
}

/** 検索語と条件を比べるための文字列。 */
function requestKey(q: string, c: SearchConditions): string {
  return JSON.stringify([q, c.since ?? "", c.until ?? "", c.user?.handle ?? "", c.server ?? "", c.tag ?? ""]);
}

function toApiConditions(c: SearchConditions): PostSearchConditions {
  return {
    since: c.since ? dayStartIso(c.since) : undefined,
    until: c.until ? dayStartIso(c.until, 1) : undefined,
    user_id: c.user?.actorId,
    server: c.server,
    tag: c.tag,
  };
}

export default function SearchPage() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();
  const { notifPanelScroll, setNotifPanelScroll, notifPanelCache, setNotifPanelCache } =
    useRightPane();
  const rightPaneRef = useRef<HTMLDivElement>(null);
  // Home画面と同じ`.rightScroll`独立スクロールコンテナのscrollTopを保存・復元する。
  const getNotifScrollContainer = useCallback(
    () => (rightPaneRef.current?.parentElement as HTMLElement | null) ?? null,
    []
  );

  const isMobile = useIsMobileViewport();
  const q = (searchParams.get("q") ?? "").trim();
  const tab: SearchTab = searchParams.get("type") === "users" ? "users" : "posts";
  const urlConditions = useMemo(() => conditionsFromParams(searchParams), [searchParams]);

  const inputRef = useRef<HTMLInputElement>(null);
  const [input, setInput] = useState(q);
  useEffect(() => setInput(q), [q]);
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // 条件のユーザーは URL にハンドルだけを載せるので、表示名と ID を手元に持つ。
  // URL から開いたとき（再読み込み・共有されたリンク）はプロフィールから引き直す。
  const [conditionUser, setConditionUser] = useState<SearchConditionUser | undefined>(undefined);
  const [userResolveFailed, setUserResolveFailed] = useState(false);
  const userHandle = urlConditions.userHandle;
  useEffect(() => {
    setUserResolveFailed(false);
    if (!userHandle) {
      setConditionUser(undefined);
      return;
    }
    if (conditionUser?.handle === userHandle) return;
    let cancelled = false;
    api.users
      .profile(userHandle)
      .then((p) => {
        if (cancelled) return;
        if (!p.actor_id) {
          setUserResolveFailed(true);
          return;
        }
        setConditionUser({
          actorId: p.actor_id,
          handle: userHandle,
          displayName: p.display_name,
          emojis: p.emojis,
        });
      })
      .catch(() => !cancelled && setUserResolveFailed(true));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [userHandle]);

  const conditions: SearchConditions = useMemo(
    () => ({
      since: urlConditions.since,
      until: urlConditions.until,
      user: conditionUser?.handle === userHandle ? conditionUser : undefined,
      server: urlConditions.server,
      tag: urlConditions.tag,
    }),
    [urlConditions, conditionUser, userHandle]
  );
  // URL にユーザーがあるのにまだ引き直せていない間は、ユーザー抜きで検索しない。
  const userPending = Boolean(userHandle) && !conditions.user && !userResolveFailed;

  // 条件欄の編集は下書きにとどめ、「検索」を押したときに検索語と一緒に確定する（ポストの
  // 検索には回数制限があり、条件を1つ足すたびに検索すると回数を無駄に使う）。URL が変わったら
  // （確定した・ほかの画面から来た）下書きを URL に合わせる。
  const [draft, setDraft] = useState<SearchConditions>(conditions);
  const urlKey = requestKey(q, { ...conditions, user: undefined }) + (userHandle ?? "");
  useEffect(() => {
    setDraft(conditions);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [urlKey, conditionUser]);

  // 検索を走らせる要求はタブごとに持ち、表示中のタブの分だけを作る。作るのは「検索」を押した
  // とき、またはほかの経路で URL の検索語・条件が変わったとき（右ペインの検索窓・共有された
  // リンク・再読み込み）だけ。タブの切り替えでは作らない（ポストの検索には回数制限があり、
  // 片方のタブで確定した検索語をもう片方へ持ち込むと、押していない検索が走って見える）。
  const [postRequest, setPostRequest] = useState<PostRequest | null>(null);
  const [userRequest, setUserRequest] = useState<UserRequest | null>(null);
  const handledUrlKey = useRef<string | null>(null);
  useEffect(() => {
    if (userPending || handledUrlKey.current === urlKey) return;
    handledUrlKey.current = urlKey;
    if (tab === "posts" && (q || hasConditions(conditions))) {
      setPostRequest((prev) => ({ seq: (prev?.seq ?? 0) + 1, q, conditions }));
    }
    if (tab === "users" && q) {
      setUserRequest((prev) => ({ seq: (prev?.seq ?? 0) + 1, q }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [urlKey, userPending]);

  /** URL を書き換える（検索語・タブ・確定した条件）。 */
  const writeUrl = (nextQ: string, nextTab: SearchTab, c: SearchConditions, handle?: string) => {
    const params = new URLSearchParams();
    if (nextQ) params.set("q", nextQ);
    if (nextTab === "users") params.set("type", "users");
    if (c.since) params.set("since", c.since);
    if (c.until) params.set("until", c.until);
    const userParam = c.user?.handle ?? handle;
    if (userParam) params.set("user", userParam);
    if (c.server) params.set("server", c.server);
    if (c.tag) params.set("tag", c.tag);
    setSearchParams(params);
  };

  function submit(e: FormEvent) {
    e.preventDefault();
    const nextQ = input.trim();
    const committed = draft;
    if (committed.user) setConditionUser(committed.user);
    // 自分で確定した URL は、上の「URL が変わったとき」の検索の対象にしない（二重に走らせない）。
    handledUrlKey.current =
      requestKey(nextQ, { ...committed, user: undefined }) + (committed.user?.handle ?? "");
    if (tab === "posts" && (nextQ || hasConditions(committed))) {
      setPostRequest((prev) => ({ seq: (prev?.seq ?? 0) + 1, q: nextQ, conditions: committed }));
    }
    if (tab === "users") {
      setUserRequest((prev) => ({ seq: (prev?.seq ?? 0) + 1, q: nextQ }));
    }
    writeUrl(nextQ, tab, committed);
  }

  // 入力中の検索語・条件が、表示中のタブの結果を出したときと違うか。違えば「検索」ボタンに
  // 投稿欄の投稿ボタンと同じ縁を付け、押せば反映されることを示す。ユーザーの検索に条件は効かない
  // ので、ユーザーのタブでは検索語だけを比べる。
  const draftQ = input.trim();
  const dirty =
    tab === "posts"
      ? postRequest
        ? requestKey(draftQ, draft) !== requestKey(postRequest.q, postRequest.conditions)
        : Boolean(draftQ) || hasConditions(draft)
      : userRequest
        ? draftQ !== userRequest.q
        : Boolean(draftQ);

  const center = (
    <>
      <form className={rp.searchForm} onSubmit={submit}>
        <input
          ref={inputRef}
          className={rp.searchInput}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder={t("search:searchPage.placeholder")}
        />
        <button
          type="submit"
          className={dirty ? `${rp.searchBtn} ${pageStyles.searchBtnDirty}` : rp.searchBtn}
          data-dirty={dirty || undefined}
        >{t("common:search")}</button>
      </form>

      <Tabs
        tabs={[t("search:searchPage.tabs.posts"), t("search:searchPage.tabs.users")]}
        active={tab === "users" ? 1 : 0}
        onChange={(i) => {
          const next: SearchTab = i === 1 ? "users" : "posts";
          // 選択中のタブをもう一度押したら、ホームのタブと同じく先頭へスクロールする。
          if (next === tab) window.scrollTo({ top: 0, behavior: "smooth" });
          else writeUrl(q, next, conditions, userHandle);
        }}
        sticky
        top={0}
      />

      {/* 結果はタブごとに持ち、切り替えても残す（戻るたびに検索し直すと回数を使う）。 */}
      <div hidden={tab !== "posts"}>
        <SearchConditionsField value={draft} onChange={setDraft} />
        {userResolveFailed && <p className={panel.message}>{t("search:searchPage.conditions.userNotFound")}</p>}
        <PostResults request={postRequest} />
      </div>
      <div hidden={tab !== "users"}>
        <UserResults request={userRequest} />
      </div>
    </>
  );

  const right = (
    <div ref={rightPaneRef}>
      <Tabs tabs={[t("search:searchPage.tabs.quickNotifications")]} active={0} onChange={() => {}} />
      <NotificationsPanel
        scroll={notifPanelScroll}
        onScrollChange={setNotifPanelScroll}
        getScrollContainer={getNotifScrollContainer}
        cache={notifPanelCache}
        onCacheChange={setNotifPanelCache}
      />
    </div>
  );

  // スマホでは右ペインが中央の下に縦積みになり、検索結果の下にクイック通知が続くだけになる
  // （通知は下部の🔔から開ける）ので出さない。
  return <AppShell center={center} right={isMobile ? undefined : right} />;
}

/** 「ポスト」タブの結果。`request` が変わったとき（「検索」を押したとき等）だけ検索し、末尾まで
 * スクロールすると続きを読み込む。続きが無くなったら（応答に `session_id` が無い）末尾に
 * 「これですべてです」を出す。 */
function PostResults({ request }: { request: PostRequest | null }) {
  const { t } = useTranslation();
  const [notes, setNotes] = useState<Note[]>([]);
  const [sessionId, setSessionId] = useState<string | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  // 続きを読み込み終えるたびに進める。1ページ分がすべて見えない投稿で0件増えたときも
  // 読み込み関数を作り直し、末尾がまだ画面内にあれば続けて読み込ませる。
  const [loadedPages, setLoadedPages] = useState(0);
  const [error, setError] = useState("");
  const [searched, setSearched] = useState(false);
  const loadingMoreRef = useRef(false);
  const seqRef = useRef(0);
  const apiConditions = useMemo(
    () => (request ? toApiConditions(request.conditions) : {}),
    [request]
  );

  useEffect(() => {
    if (!request) return;
    seqRef.current = request.seq;
    setSessionId(undefined);
    setError("");
    let cancelled = false;
    setLoading(true);
    api.notes
      .search({ q: request.q, limit: PAGE_SIZE, ...apiConditions })
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request?.seq]);

  const loadMore = useCallback(async () => {
    if (!request || !sessionId || loadingMoreRef.current) return;
    const seq = request.seq;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    try {
      const res = await api.notes.search({ q: request.q, limit: PAGE_SIZE, session_id: sessionId, ...apiConditions });
      if (seqRef.current !== seq) return;
      setNotes((prev) => {
        const seen = new Set(prev.map((n) => n.id));
        return [...prev, ...res.notes.filter((n) => !seen.has(n.id))];
      });
      setSessionId(res.session_id);
    } catch (e) {
      if (seqRef.current !== seq) return;
      setError(getErrorMessage(e));
      // 失敗したら自動では読み込み直さない（同じ失敗を繰り返さないよう、続きを打ち切る）。
      setSessionId(undefined);
    } finally {
      loadingMoreRef.current = false;
      setLoadingMore(false);
      setLoadedPages((n) => n + 1);
    }
    // loadedPages は、0件増えたときも関数を作り直して末尾の監視をつけ直すために入れる。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request, sessionId, apiConditions, loadedPages]);

  const hasMore = Boolean(sessionId);
  return (
    <>
      {error && <p className={panel.message}>{error}</p>}
      <NoteList
        notes={notes}
        loading={loading}
        emptyMessage={searched ? t("search:searchPage.noResults") : t("search:searchPage.prompt")}
        onLoadMore={loadMore}
        hasMore={hasMore}
        loadingMore={loadingMore}
      />
      {searched && !loading && !hasMore && !error && notes.length > 0 && (
        <p className={panel.message}>{t("search:searchPage.end")}</p>
      )}
    </>
  );
}

/** 「ユーザー」タブの結果（表示名・ハンドル・自己紹介の一致）。`request` が変わったとき
 * （「検索」を押したとき等）だけ検索し、末尾までスクロールすると続きを読み込む。 */
function UserResults({ request }: { request: UserRequest | null }) {
  const q = request?.q ?? "";
  const { t } = useTranslation();
  const [users, setUsers] = useState<ActorSuggestion[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState("");
  const [searched, setSearched] = useState(false);
  const loadingMoreRef = useRef(false);
  const seqRef = useRef(0);

  useEffect(() => {
    seqRef.current = request?.seq ?? 0;
    setError("");
    setHasMore(false);
    if (!q) {
      setUsers([]);
      setSearched(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    api.actors
      .searchUsers(q, { limit: PAGE_SIZE })
      .then((rows) => {
        if (cancelled) return;
        setUsers(rows);
        setHasMore(rows.length >= PAGE_SIZE);
        setSearched(true);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request?.seq]);

  const loadMore = useCallback(async () => {
    if (!q || loadingMoreRef.current) return;
    const seq = seqRef.current;
    loadingMoreRef.current = true;
    setLoadingMore(true);
    try {
      const rows = await api.actors.searchUsers(q, { limit: PAGE_SIZE, offset: users.length });
      if (seqRef.current !== seq) return;
      setUsers((prev) => [...prev, ...rows.filter((r) => !prev.some((p) => p.actor_id === r.actor_id))]);
      setHasMore(rows.length >= PAGE_SIZE);
    } catch (e) {
      if (seqRef.current !== seq) return;
      setError(getErrorMessage(e));
      setHasMore(false);
    } finally {
      loadingMoreRef.current = false;
      setLoadingMore(false);
    }
  }, [q, users.length]);
  const sentinelRef = useInfiniteScrollSentinel<HTMLDivElement>(loadMore, hasMore);

  if (loading) return <p className={panel.message}>{t("common:loading")}</p>;
  return (
    <>
      {error && <p className={panel.message}>{error}</p>}
      {users.length === 0 ? (
        <p className={panel.message}>
          {searched ? t("search:searchPage.users.noResults") : t("search:searchPage.users.prompt")}
        </p>
      ) : (
        <div className={listStyles.list}>
          {users.map((u) => (
            <Link key={u.actor_id} to={profilePath(u.username, u.domain)} className={listStyles.row}>
              <Avatar url={u.avatar_url} name={u.display_name || u.username} size={40} />
              <div className={listStyles.names}>
                <span className={listStyles.displayName}>
                  <DisplayName name={u.display_name} fallback={u.username} emojis={u.emojis} />
                </span>
                <span className={listStyles.acct}>{u.target}</span>
              </div>
            </Link>
          ))}
        </div>
      )}
      {hasMore && (
        <div ref={sentinelRef} className={listStyles.sentinel}>
          {loadingMore ? t("common:loading") : ""}
        </div>
      )}
      {searched && !hasMore && !error && users.length > 0 && (
        <p className={panel.message}>{t("search:searchPage.end")}</p>
      )}
    </>
  );
}
