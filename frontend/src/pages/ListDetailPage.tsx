import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, ApiError, getErrorMessage, ListDetail, Note } from "../api/client";
import AppShell from "../components/layout/AppShell";
import NoteList from "../components/note/NoteList";
import panel from "../components/common/Panel.module.css";
import styles from "./ListDetailPage.module.css";

export default function ListDetailPage() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();

  const [detail, setDetail] = useState<ListDetail | null>(null);
  const [notes, setNotes] = useState<Note[]>([]);
  const [loading, setLoading] = useState(true);
  const [notFound, setNotFound] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setLoading(true);
    setNotFound(false);
    setError("");
    Promise.all([api.lists.get(id), api.lists.timeline(id, { limit: 30 })])
      .then(([d, n]) => {
        if (cancelled) return;
        setDetail(d);
        setNotes(n);
      })
      .catch((e) => {
        if (cancelled) return;
        if (e instanceof ApiError && e.code === "LIST_NOT_FOUND") {
          setNotFound(true);
        } else {
          setError(getErrorMessage(e));
        }
      })
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [id]);

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={() => navigate(-1)}>
          ← 戻る
        </button>
        <span className={panel.title}>{detail ? detail.name : "リスト"}</span>
      </header>

      {loading ? (
        <p className={panel.message}>読み込み中...</p>
      ) : notFound ? (
        <p className={panel.message}>このリストは存在しないか、非公開です。</p>
      ) : error ? (
        <p className={panel.message}>{error}</p>
      ) : detail ? (
        <>
          <div className={styles.meta}>
            <span>{detail.members.length}人のメンバー</span>
            {detail.is_owner && (
              <Link to="/settings/lists" className={styles.editLink}>
                編集
              </Link>
            )}
          </div>

          <div className={styles.membersRow}>
            {detail.members.map((m) => (
              <span key={m.actor_id} className={styles.memberBadge}>
                {m.avatar_url ? <img src={m.avatar_url} alt="" /> : <span>{(m.display_name || m.username)[0]?.toUpperCase()}</span>}
                {m.display_name || m.username}
              </span>
            ))}
          </div>

          <NoteList notes={notes} emptyMessage="このリストのメンバーの投稿がここに表示されます。" />
        </>
      ) : null}
    </>
  );

  return <AppShell center={center} />;
}
