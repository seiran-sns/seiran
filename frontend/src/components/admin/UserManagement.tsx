import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, AdminUser, getErrorMessage } from "../../api/client";
import { useCursorPagination } from "../../hooks/useCursorPagination";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

const ROLES = ["user", "emoji-editor", "moderator", "admin"];
const PAGE_SIZE = 30;

export default function UserManagement() {
  const { t } = useTranslation();
  const [searchInput, setSearchInput] = useState("");
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);

  const fetchPage = useCallback(
    (afterId: string) =>
      api.admin.listUsers({ q: query || undefined, afterId, limit: PAGE_SIZE }),
    [query]
  );
  const {
    items: users,
    setItems: setUsers,
    hasMore,
    setHasMore,
    loadingMore,
    loadMore,
  } = useCursorPagination<AdminUser>(
    fetchPage,
    (u) => u.id,
    PAGE_SIZE,
    (e) => setError(getErrorMessage(e))
  );
  const sentinelRef = useInfiniteScrollSentinel<HTMLDivElement>(loadMore, hasMore);

  // 絞り込み入力のデバウンス（リスト機能のメンバー追加サジェストと同じ300ms、
  // useListsSettings.ts のロジックを流用）。
  useEffect(() => {
    const timer = window.setTimeout(() => setQuery(searchInput.trim()), 300);
    return () => window.clearTimeout(timer);
  }, [searchInput]);

  // 初回ロード、および絞り込み文字列が変わるたびに1ページ目から取り直す。
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError("");
    api.admin
      .listUsers({ q: query || undefined, limit: PAGE_SIZE })
      .then((rows) => {
        if (cancelled) return;
        setUsers(rows);
        setHasMore(rows.length >= PAGE_SIZE);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query]);

  async function toggleSuspend(u: AdminUser) {
    setBusyId(u.id);
    setError("");
    try {
      if (u.suspended_at) await api.admin.unsuspendUser(u.id);
      else await api.admin.suspendUser(u.id);
      setUsers((prev) =>
        prev.map((x) =>
          x.id === u.id
            ? { ...x, suspended_at: u.suspended_at ? null : new Date().toISOString() }
            : x
        )
      );
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  async function changeRole(u: AdminUser, role: string) {
    if (role === u.role) return;
    setBusyId(u.id);
    setError("");
    try {
      await api.admin.changeUserRole(u.id, role);
      setUsers((prev) => prev.map((x) => (x.id === u.id ? { ...x, role } : x)));
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  async function disableTotp(u: AdminUser) {
    if (!window.confirm(t("admin:userManagement.disableTotpConfirm", { username: u.username ?? u.email }))) return;
    setBusyId(u.id);
    setError("");
    try {
      await api.admin.disableUserTotp(u.id);
      setUsers((prev) => prev.map((x) => (x.id === u.id ? { ...x, totp_enabled: false } : x)));
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>{t("admin:userManagement.title")}</h2>
      <input
        className={`${styles.input} ${styles.searchBox}`}
        type="text"
        value={searchInput}
        onChange={(e) => setSearchInput(e.target.value)}
        placeholder={t("admin:userManagement.searchPlaceholder")}
      />
      {error && <p className={styles.error}>{error}</p>}
      {loading ? (
        <p className={panel.message}>{t("common:loading")}</p>
      ) : (
        <div className={styles.card}>
          {users.length === 0 && <p className={panel.message}>{t("admin:userManagement.emptyMessage")}</p>}
          {users.map((u) => (
            <div key={u.id} className={styles.row}>
              <div className={styles.avatar}>
                {u.avatar_url ? (
                  <img src={u.avatar_url} alt="" />
                ) : (
                  <span>{(u.display_name || u.username || u.email)[0]?.toUpperCase() ?? "?"}</span>
                )}
              </div>
              <div className={styles.grow}>
                <div className={styles.primaryText}>
                  {u.display_name || u.username || t("admin:userManagement.noActorLabel")}
                </div>
                <div className={styles.subText}>
                  {u.username ? `@${u.username} · ` : ""}
                  {u.email}
                </div>
                <div className={styles.authStatus}>
                  <span>{t(u.totp_enabled ? "admin:userManagement.totpEnabled" : "admin:userManagement.totpDisabled")}</span>
                  <span>{t("admin:userManagement.passkeyCount", { count: u.passkey_count })}</span>
                </div>
              </div>
              {u.suspended_at && (
                <span className={`${styles.badge} ${styles.badgeSuspended}`}>{t("admin:userManagement.suspendedBadge")}</span>
              )}
              <select
                className={styles.select}
                value={u.role}
                disabled={busyId === u.id}
                onChange={(e) => changeRole(u, e.target.value)}
              >
                {ROLES.map((r) => (
                  <option key={r} value={r}>
                    {r}
                  </option>
                ))}
              </select>
              <button
                className={u.suspended_at ? styles.btnGhost : styles.btnDanger}
                disabled={busyId === u.id}
                onClick={() => toggleSuspend(u)}
              >
                {u.suspended_at ? t("admin:userManagement.unsuspendButton") : t("admin:userManagement.suspendButton")}
              </button>
              {u.totp_enabled && (
                <button
                  className={styles.btnDanger}
                  disabled={busyId === u.id}
                  onClick={() => disableTotp(u)}
                >
                  {t("admin:userManagement.disableTotpButton")}
                </button>
              )}
            </div>
          ))}
        </div>
      )}
      {hasMore && (
        <div ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? t("common:loading") : ""}
        </div>
      )}
    </div>
  );
}
