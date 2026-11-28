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
import { useIsMobileViewport } from "../hooks/useIsNarrowViewport";
import { profilePath } from "../lib/format";
import panel from "../components/common/Panel.module.css";
import rp from "../components/right/RightPanels.module.css";
import listStyles from "../components/right/FollowListPanel.module.css";
import styles from "./HomePage.module.css";

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

  /** 検索語・タブ・条件を URL に書く（URL が変わると検索し直す）。 */
  const navigate = (next: { q?: string; tab?: SearchTab; conditions?: SearchConditions }) => {
    const nextQ = next.q ?? q;
    const nextTab = next.tab ?? tab;
    const c = next.conditions ?? conditions;
    const params = new URLSearchParams();
    if (nextQ) params.set("q", nextQ);
    if (nextTab === "users") params.set("type", "users");
    if (c.since) params.set("since", c.since);
    if (c.until) params.set("until", c.until);
    if (c.user) params.set("user", c.user.handle);
    else if (next.conditions === undefined && userHandle) params.set("user", userHandle);
    if (c.server) params.set("server", c.server);
    if (c.tag) params.set("tag", c.tag);
    if (next.conditions?.user) setConditionUser(next.conditions.user);
    setSearchParams(params);
  };

  function submit(e: FormEvent) {
    e.preventDefault();
    navigate({ q: input.trim() });
  }

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
        <button type="submit" className={rp.searchBtn}>{t("common:search")}</button>
      </form>

      <Tabs
        tabs={[t("search:searchPage.tabs.posts"), t("search:searchPage.tabs.users")]}
        active={tab === "users" ? 1 : 0}
        onChange={(i) => navigate({ tab: i === 1 ? "users" : "posts" })}
      />

      {tab === "posts" ? (
        <>
          <SearchConditionsField value={conditions} onChange={(c) => navigate({ conditions: c })} />
          {userResolveFailed && <p className={panel.message}>{t("search:searchPage.conditions.userNotFound")}</p>}
          <PostResults q={q} conditions={conditions} pending={userPending} />
        </>
      ) : (
        <UserResults q={q} />
      )}
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

/** 「ポスト」タブの結果。検索語か条件のどちらかがあれば検索する。 */
function PostResults({ q, conditions, pending }: { q: string; conditions: SearchConditions; pending: boolean }) {
  const { t } = useTranslation();
  const [notes, setNotes] = useState<Note[]>([]);
  const [sessionId, setSessionId] = useState<string | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState("");
  const [searched, setSearched] = useState(false);
  const apiConditions = useMemo(() => toApiConditions(conditions), [conditions]);
  const active = !pending && (Boolean(q) || hasConditions(conditions));
  const key = JSON.stringify([q, apiConditions]);

  useEffect(() => {
    setSessionId(undefined);
    setError("");
    if (!active) {
      setNotes([]);
      setSearched(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    api.notes
      .search({ q, limit: PAGE_SIZE, ...apiConditions })
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
  }, [key, active]);

  async function loadMore() {
    if (!active || loadingMore) return;
    setLoadingMore(true);
    try {
      const res = await api.notes.search({ q, limit: PAGE_SIZE, session_id: sessionId, ...apiConditions });
      setNotes((prev) => [...prev, ...res.notes]);
      setSessionId(res.session_id);
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setLoadingMore(false);
    }
  }

  return (
    <>
      {error && <p className={panel.message}>{error}</p>}
      <NoteList
        notes={notes}
        loading={loading || pending}
        emptyMessage={searched ? t("search:searchPage.noResults") : t("search:searchPage.prompt")}
      />
      {searched && !loading && notes.length > 0 && (
        <div className={styles.feedTabs} style={{ position: "static" }}>
          <button
            className={styles.feedTab}
            onClick={loadMore}
            disabled={loadingMore}
            style={{ cursor: loadingMore ? "default" : "pointer" }}
          >
            {loadingMore ? t("common:loading") : t("search:searchPage.loadMoreButton")}
          </button>
        </div>
      )}
    </>
  );
}

/** 「ユーザー」タブの結果（表示名・ハンドル・自己紹介の一致）。 */
function UserResults({ q }: { q: string }) {
  const { t } = useTranslation();
  const [users, setUsers] = useState<ActorSuggestion[]>([]);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState("");
  const [searched, setSearched] = useState(false);

  useEffect(() => {
    setError("");
    if (!q) {
      setUsers([]);
      setSearched(false);
      setHasMore(false);
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
  }, [q]);

  async function loadMore() {
    if (loadingMore) return;
    setLoadingMore(true);
    try {
      const rows = await api.actors.searchUsers(q, { limit: PAGE_SIZE, offset: users.length });
      setUsers((prev) => [...prev, ...rows.filter((r) => !prev.some((p) => p.actor_id === r.actor_id))]);
      setHasMore(rows.length >= PAGE_SIZE);
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setLoadingMore(false);
    }
  }

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
        <div className={styles.feedTabs} style={{ position: "static" }}>
          <button
            className={styles.feedTab}
            onClick={loadMore}
            disabled={loadingMore}
            style={{ cursor: loadingMore ? "default" : "pointer" }}
          >
            {loadingMore ? t("common:loading") : t("search:searchPage.loadMoreButton")}
          </button>
        </div>
      )}
    </>
  );
}
