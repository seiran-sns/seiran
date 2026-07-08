import { useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { api, UserProfile, getErrorMessage } from "../api/client";
import Modal from "../components/common/Modal";
import AppShell from "../components/layout/AppShell";
import NoteCard from "../components/note/NoteCard";
import { useAuth } from "../contexts/AuthContext";
import panel from "../components/common/Panel.module.css";
import styles from "./ProfilePage.module.css";

type FollowStatus = "not_following" | "pending" | "accepted";

export default function ProfilePage() {
  const [searchParams] = useSearchParams();
  const { acct } = useParams<{ acct: string }>();
  const navigate = useNavigate();
  // permalink `/@handle`（#36）を優先し、旧 `/profile?q=` も後方互換で受ける。
  const q = acct ? acct.replace(/^@/, "") : searchParams.get("q") ?? "";

  const [profile, setProfile] = useState<UserProfile | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [followStatus, setFollowStatus] = useState<FollowStatus>("not_following");
  const [following, setFollowing] = useState(false);
  const [bridgeModalOpen, setBridgeModalOpen] = useState(false);

  useEffect(() => {
    if (!q) return;
    let cancelled = false;
    setLoading(true);
    setError("");
    api.users
      .profile(q)
      .then((p) => {
        if (cancelled) return;
        setProfile(p);
        setFollowStatus(p.follow_status);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [q]);

  const { user } = useAuth();
  const isLocal = profile?.actor_type === "local";
  const isBridge = !!profile?.bridge_real_handle;
  const isSelf = isLocal && !!user && user.username === profile?.username;

  async function doFollow() {
    if (!profile) return;
    setFollowing(true);
    try {
      // AP ユーザーは ap_uri、Bsky ユーザーは at_did（DID）をターゲットとして渡す
      const target = profile.ap_uri || profile.at_did || `${profile.username}@${profile.domain}`;
      await api.follows.create(target);
      setFollowStatus("pending");
    } catch (e) {
      alert(getErrorMessage(e));
    } finally {
      setFollowing(false);
    }
  }

  // フォロー時のインターセプト（Doc5 §3.2）: 影武者なら確認モーダルを割り込ませる。
  function handleFollowClick() {
    if (isBridge) {
      setBridgeModalOpen(true);
    } else {
      doFollow();
    }
  }

  function warpToReal() {
    if (profile?.bridge_real_handle) {
      setBridgeModalOpen(false);
      navigate(`/${profile.bridge_real_handle}`);
    }
  }

  const center = (
    <>
      <header className={panel.header}>
        <button className={panel.backBtn} onClick={() => navigate(-1)}>
          ← 戻る
        </button>
        <span className={panel.title}>プロフィール</span>
      </header>

      {loading && <p className={panel.message}>読み込み中...</p>}
      {error && <p className={panel.message}>{error}</p>}

      {profile && (
        <div className={styles.card}>
          {/* 本尊ワープ（Doc5 §3.1）: 影武者なら最も目立つ位置に強制表示 */}
          {isBridge && (
            <button className={styles.warpBanner} onClick={warpToReal}>
              <span className={styles.warpIcon}>
                {profile.bridge_protocol === "bsky" ? "🦋" : "🌐"}
              </span>
              <span>
                このアカウントは<strong>影武者</strong>です。本尊（{profile.bridge_real_handle}）はこちら →
              </span>
            </button>
          )}

          <div className={styles.avatarLarge}>
            {(profile.display_name || profile.username)[0]?.toUpperCase() ?? "?"}
          </div>

          <div className={styles.names}>
            <span className={styles.displayName}>{profile.display_name || profile.username}</span>
            <span className={styles.acct}>
              @{profile.username}
              {profile.domain && profile.domain !== window.location.hostname && `@${profile.domain}`}
            </span>
          </div>

          <div className={styles.badges}>
            {profile.is_paired && (
              <span className={`${styles.badge} ${styles.pairedBadge}`} title="リモート seiran ユーザーと魂の結合済み">
                🀄 魂の結合済み
              </span>
            )}
            {profile.at_did && (
              <span className={styles.badge}>🦋 Bluesky</span>
            )}
            {!isLocal && profile.actor_type === "fedi" && (
              <span className={styles.badge}>🌐 Fediverse</span>
            )}
          </div>

          {profile.bio && <p className={styles.bio}>{profile.bio}</p>}

          {/* プロトコルアイデンティティ */}
          <div className={styles.identity}>
            {profile.at_did && (
              <div className={styles.idRow}>
                <span className={styles.idLabel}>DID</span>
                <span className={styles.idValue}>{profile.at_did}</span>
              </div>
            )}
            {profile.ap_uri && (
              <div className={styles.idRow}>
                <span className={styles.idLabel}>URI</span>
                <a className={styles.idValue} href={profile.ap_uri} target="_blank" rel="noopener noreferrer">
                  {profile.ap_uri}
                </a>
              </div>
            )}
          </div>

          {isSelf && (
            <div className={styles.followArea}>
              <button className={styles.editBtn} onClick={() => navigate("/settings/profile")}>
                プロフィールを編集
              </button>
            </div>
          )}

          {!isLocal && (
            <div className={styles.followArea}>
              {followStatus === "accepted" && <span className={styles.followingBadge}>フォロー中</span>}
              {followStatus === "pending" && <span className={styles.pendingBadge}>承認待ち</span>}
              {followStatus === "not_following" && (
                <button className={styles.followBtn} onClick={handleFollowClick} disabled={following}>
                  {following ? "送信中..." : "フォロー"}
                </button>
              )}
            </div>
          )}
        </div>
      )}
    </>
  );

  const right = profile && (
    <>
      <div className={panel.rightHeader}>投稿</div>
      {profile.recent_posts.length === 0 ? (
        <p className={panel.message}>投稿がありません。</p>
      ) : (
        profile.recent_posts.map((post) => <NoteCard key={post.id} note={post} />)
      )}
    </>
  );

  return (
    <>
      <AppShell center={center} right={right} />

      <Modal
        open={bridgeModalOpen}
        onClose={() => setBridgeModalOpen(false)}
        title="影武者アカウントのフォロー"
      >
        <p className={styles.modalText}>
          このアカウントは、
          {profile?.bridge_protocol === "bsky" ? "Bluesky" : "Fediverse"}
          のユーザーがブリッジ経由で投影されている<strong>影武者</strong>です。
          seiran の機能をフルに活用するため、本尊（オリジナルアカウント）を直接フォローすることをおすすめします。
        </p>
        <div className={styles.modalActions}>
          <button className={styles.modalPrimary} onClick={warpToReal}>
            本尊のプロフィールへ移動
          </button>
          <button
            className={styles.modalSecondary}
            onClick={() => {
              setBridgeModalOpen(false);
              doFollow();
            }}
          >
            そのまま影武者をフォロー
          </button>
        </div>
      </Modal>
    </>
  );
}
