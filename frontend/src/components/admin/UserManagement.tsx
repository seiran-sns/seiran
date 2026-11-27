import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, AdminUser, getErrorMessage } from "../../api/client";
import { useCursorPagination } from "../../hooks/useCursorPagination";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";
import { isPrivilegedRole } from "../../lib/roles";
import { isEditConflict } from "../../lib/editDiff";
import ConfirmModal from "../common/ConfirmModal";
import DisplayName from "../common/DisplayName";

const ROLES = ["quarantine", "silence", "user", "emoji-editor", "moderator", "admin"];
const PAGE_SIZE = 30;

export default function UserManagement() {
  const { t } = useTranslation();
  const [searchInput, setSearchInput] = useState("");
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);
  /** 別の画面との衝突で保存できなかったユーザーの、サーバーに保存されているロール。画面には
   * 選んだロールを残すので、選び直しの差分の基準はこちらを使う（一覧を取り直したら消す）。 */
  const savedRoles = useRef(new Map<string, string>());

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
        savedRoles.current.clear();
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

  // 凍結・凍結解除・ロール変更は確認してから行う。
  const [pending, setPending] = useState<
    { kind: "suspend"; user: AdminUser } | { kind: "role"; user: AdminUser; role: string } | null
  >(null);
  const userLabel = (u: AdminUser) => (u.username ? `@${u.username}` : u.email);
  async function runPending() {
    if (!pending) return;
    if (pending.kind === "suspend") await toggleSuspend(pending.user);
    else await changeRole(pending.user, pending.role);
    setPending(null);
  }

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
    const from = savedRoles.current.get(u.id) ?? u.role;
    try {
      await api.admin.changeUserRole(u.id, { from, to: role });
      savedRoles.current.delete(u.id);
      setUsers((prev) => prev.map((x) => (x.id === u.id ? { ...x, role } : x)));
    } catch (e) {
      if (isEditConflict(e)) {
        // 別の画面でロールが変えられていた: 選んだロールは画面に残してエラーだけ出す
        savedRoles.current.set(u.id, from);
        setUsers((prev) => prev.map((x) => (x.id === u.id ? { ...x, role } : x)));
      }
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
                  {u.display_name || u.username ? (
                    <DisplayName name={u.display_name} fallback={u.username ?? ""} emojis={u.emojis} />
                  ) : (
                    t("admin:userManagement.noActorLabel")
                  )}
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
              {u.withdrawn_at && (
                <span className={`${styles.badge} ${styles.badgeWithdrawn}`}>{t("admin:userManagement.withdrawnBadge")}</span>
              )}
              {u.suspended_at && (
                <span className={`${styles.badge} ${styles.badgeSuspended}`}>{t("admin:userManagement.suspendedBadge")}</span>
              )}
              <select
                className={styles.select}
                value={u.role}
                disabled={busyId === u.id}
                onChange={(e) => {
                  if (e.target.value !== u.role) setPending({ kind: "role", user: u, role: e.target.value });
                }}
              >
                {ROLES.map((r) => (
                  <option key={r} value={r}>
                    {r}
                  </option>
                ))}
              </select>
              {/* admin・moderator は凍結できない（凍結済みなら解除だけはできる）。 */}
              {(u.suspended_at || !isPrivilegedRole(u.role)) && (
                <button
                  className={u.suspended_at ? styles.btnGhost : styles.btnDanger}
                  disabled={busyId === u.id}
                  onClick={() => setPending({ kind: "suspend", user: u })}
                >
                  {u.suspended_at ? t("admin:userManagement.unsuspendButton") : t("admin:userManagement.suspendButton")}
                </button>
              )}
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
      {pending && (
        <ConfirmModal
          open
          onClose={() => setPending(null)}
          title={
            pending.kind === "role"
              ? t("admin:userManagement.changeRoleTitle")
              : pending.user.suspended_at
                ? t("admin:userManagement.unsuspendButton")
                : t("admin:userManagement.suspendButton")
          }
          confirmLabel={
            pending.kind === "role"
              ? t("admin:userManagement.changeRoleTitle")
              : pending.user.suspended_at
                ? t("admin:userManagement.unsuspendButton")
                : t("admin:userManagement.suspendButton")
          }
          onConfirm={() => void runPending()}
          busy={busyId === pending.user.id}
        >
          <p>
            {pending.kind === "role"
              ? t("admin:userManagement.changeRoleConfirm", {
                  user: userLabel(pending.user),
                  from: pending.user.role,
                  to: pending.role,
                })
              : t(
                  pending.user.suspended_at
                    ? "admin:userManagement.unsuspendConfirm"
                    : "admin:userManagement.suspendConfirm",
                  { user: userLabel(pending.user) },
                )}
          </p>
        </ConfirmModal>
      )}
    </div>
  );
}
