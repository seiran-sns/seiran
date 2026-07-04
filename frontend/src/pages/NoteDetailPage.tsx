import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { api, Note, getErrorMessage } from "../api/client";
import Tabs from "../components/common/Tabs";
import AppShell from "../components/layout/AppShell";
import NoteCard from "../components/note/NoteCard";
import { acct, displayName, formatDate, profileQuery, protocolBadge } from "../lib/format";
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
  const [before, setBefore] = useState<Note[]>([]);
  const [after, setAfter] = useState<Note[]>([]);
  const [ctxLoading, setCtxLoading] = useState(true);

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

  // 右ペイン「投稿主の前後の投稿」用のコンテキストを取得（Doc5 §2.3 モード1）。
  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setCtxLoading(true);
    setBefore([]);
    setAfter([]);
    api.notes
      .context(id)
      .then((ctx) => {
        if (cancelled) return;
        setBefore(ctx.before);
        setAfter(ctx.after);
      })
      .catch(() => {})
      .finally(() => !cancelled && setCtxLoading(false));
    return () => {
      cancelled = true;
    };
  }, [id]);

  const badge = note ? protocolBadge(note.user.actorType) : null;

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
          {/* リプライ元への垂直遡り（Doc5 §2.3） */}
          {note.replyId && (
            <Link to={`/notes/${note.replyId}`} className={styles.upLink}>
              ↩ 返信元のポストを見る
            </Link>
          )}

          {/* 投稿主の前の投稿（右ペインが隠れる幅でのみ中央に表示） */}
          {before.length > 0 && (
            <section className={styles.narrowContext}>
              <div className={styles.contextLabel}>投稿主の前の投稿</div>
              {[...before].reverse().map((n) => (
                <NoteCard key={n.id} note={n} />
              ))}
            </section>
          )}

          <article className={styles.focal}>
            <button
              className={styles.focalUser}
              onClick={() =>
                navigate(`/profile?q=${encodeURIComponent(profileQuery(note.user.username, note.user.domain))}`)
              }
            >
              <span className={styles.focalAvatar}>
                {(note.user.displayName || note.user.username)[0]?.toUpperCase() ?? "?"}
              </span>
              <span className={styles.focalNames}>
                <span className={styles.focalDisplayName}>{displayName(note)}</span>
                <span className={styles.focalAcct}>
                  {acct(note)}
                  {badge && <span title={badge.label}> {badge.icon}</span>}
                </span>
              </span>
            </button>

            <p className={styles.focalBody}>{note.text}</p>

            {note.attachments && note.attachments.length > 0 && (
              <div className={styles.focalAttachments}>
                {note.attachments.map((att, i) => (
                  <a key={i} href={att.url} target="_blank" rel="noopener noreferrer">
                    <img src={att.url} alt="" className={styles.focalAttachImage} loading="lazy" />
                  </a>
                ))}
              </div>
            )}

            <time className={styles.focalTime}>{formatDate(note.createdAt)}</time>

            {note.quoteId && (
              <Link to={`/notes/${note.quoteId}`} className={styles.quoteLink}>
                ❝ 引用元のポストを見る
              </Link>
            )}
            {note.parentOriginalId && (
              <Link to={`/notes/${note.parentOriginalId}`} className={styles.originalLink}>
                🀄 本尊のオリジナル投稿を見る
              </Link>
            )}
          </article>

          {/* 投稿主の次の投稿（右ペインが隠れる幅でのみ中央に表示） */}
          {after.length > 0 && (
            <section className={styles.narrowContext}>
              <div className={styles.contextLabel}>投稿主の次の投稿</div>
              {after.map((n) => (
                <NoteCard key={n.id} note={n} />
              ))}
            </section>
          )}

          {/* 直系リプライ・引用（専用 API 未実装のためプレースホルダ） */}
          <div className={panel.placeholder}>
            <span className={panel.placeholderIcon}>💬</span>
            直系リプライ・引用ポストのツリー表示は準備中です。
          </div>
        </>
      )}
    </>
  );

  const contextList = [...before].reverse().concat(after);

  const right = (
    <>
      <Tabs
        tabs={["投稿主の前後", "リアクション"]}
        active={noteDetailTab}
        onChange={setNoteDetailTab}
      />
      {noteDetailTab === 0 ? (
        ctxLoading ? (
          <p className={panel.message}>読み込み中...</p>
        ) : contextList.length === 0 ? (
          <p className={panel.message}>前後の投稿はありません。</p>
        ) : (
          contextList.map((n) => (
            <div key={n.id} className={n.id === id ? styles.ctxCurrent : ""}>
              <NoteCard note={n} />
            </div>
          ))
        )
      ) : (
        <div className={panel.placeholder}>
          <span className={panel.placeholderIcon}>😀</span>
          絵文字リアクション（Fedi カスタム絵文字・ATP Like 等）の集計表示は準備中です。
        </div>
      )}
    </>
  );

  return <AppShell center={center} right={right} />;
}
