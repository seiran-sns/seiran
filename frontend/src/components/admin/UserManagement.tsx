import { useEffect, useState } from "react";
import { api, AdminUser, getErrorMessage } from "../../api/client";
import panel from "../common/Panel.module.css";
import styles from "../../pages/Admin.module.css";

const ROLES = ["user", "moderator", "admin"];

export default function UserManagement() {
  const [users, setUsers] = useState<AdminUser[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);

  function load() {
    setLoading(true);
    setError("");
    api.admin
      .listUsers()
      .then(setUsers)
      .catch((e) => setError(getErrorMessage(e)))
      .finally(() => setLoading(false));
  }

  useEffect(load, []);

  async function toggleSuspend(u: AdminUser) {
    setBusyId(u.id);
    setError("");
    try {
      if (u.suspended_at) await api.admin.unsuspendUser(u.id);
      else await api.admin.suspendUser(u.id);
      load();
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
      load();
    } catch (e) {
      setError(getErrorMessage(e));
    } finally {
      setBusyId(null);
    }
  }

  if (loading) return <p className={panel.message}>読み込み中...</p>;

  return (
    <div className={styles.body}>
      <h2 className={styles.sectionTitle}>ユーザー管理</h2>
      {error && <p className={styles.error}>{error}</p>}
      <div className={styles.card}>
        {users.length === 0 && <p className={panel.message}>ユーザーがいません。</p>}
        {users.map((u) => (
          <div key={u.id} className={styles.row}>
            <div className={styles.grow}>
              <div className={styles.primaryText}>
                {u.username ? `@${u.username}` : "(アクター未作成)"}
              </div>
              <div className={styles.subText}>{u.email}</div>
            </div>
            {u.suspended_at && <span className={`${styles.badge} ${styles.badgeSuspended}`}>凍結中</span>}
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
              {u.suspended_at ? "凍結解除" : "凍結"}
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
