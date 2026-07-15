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
  const [unfollowing, setUnfollowing] = useState(false);
  const [bridgeModalOpen, setBridgeModalOpen] = useState(false);
  // AppShell.module.css の右ペイン非表示ブレークポイント（1400px）と合わせる。
  // 狭幅では右ペインが無いため、ピン留め・最新ポストの両方を中央ペインへ連続表示する（#61）。
  const [isNarrow, setIsNarrow] = useState(false);

  useEffect(() => {
    const mql = window.matchMedia("(max-width: 1400px)");
    setIsNarrow(mql.matches);
    const handler = (e: MediaQueryListEvent) => setIsNarrow(e.matches);
    mql.addEventListener("change", handler);
    return () => mql.removeEventListener("change", handler);
  }, []);

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

  function followTarget(): string {
    if (!profile) return "";
    // ローカルユーザーはユーザー名のみ、AP は ap_uri、Bsky は at_did（DID）
    if (profile.actor_type === "local") return profile.username;
    return profile.ap_uri || profile.at_did || `${profile.username}@${profile.domain}`;
  }

  async function doFollow() {
    if (!profile) return;
    setFollowing(true);
    try {
      const result = await api.follows.create(followTarget());
      // ローカルフォローは即 accepted
      setFollowStatus(result.status === "accepted" ? "accepted" : "pending");
    } catch (e) {
      alert(getErrorMessage(e));
    } finally {
      setFollowing(false);
    }
  }

  async function doUnfollow() {
    if (!profile) return;
    setUnfollowing(true);
    try {
      await api.follows.delete(followTarget());
      setFollowStatus("not_following");
    } catch (e) {
      alert(getErrorMessage(e));
    } finally {
      setUnfollowing(false);
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
            {profile.avatar_url
              ? <img src={profile.avatar_url} alt="" className={styles.avatarImg} />
              : (profile.display_name || profile.username)[0]?.toUpperCase() ?? "?"}
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

          {!isSelf && (
            <div className={styles.followArea}>
              {followStatus === "accepted" && (
                <>
                  <span className={styles.followingBadge}>フォロー中</span>
                  <button className={styles.unfollowBtn} onClick={doUnfollow} disabled={unfollowing}>
                    {unfollowing ? "解除中..." : "フォロー解除"}
                  </button>
                </>
              )}
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

  // ピン留めポスト（#61）: プロフィールカード直下に表示。多すぎるピン留めが最新ポスト一覧の
  // 邪魔をしないよう、最新ポストとは別セクションにする。
  const pinnedSection = profile && profile.pinned_posts.length > 0 && (
    <>
      <div className={panel.rightHeader}>ピン留め</div>
      {profile.pinned_posts.map((post) => <NoteCard key={post.id} note={post} />)}
    </>
  );

  const recentSection = profile && (
    <>
      <div className={panel.rightHeader}>投稿</div>
      {profile.recent_posts.length === 0 ? (
        <p className={panel.message}>投稿がありません。</p>
      ) : (
        profile.recent_posts.map((post) => <NoteCard key={post.id} note={post} />)
      )}
    </>
  );

  // 狭幅（スマホ等、右ペインが無い）では中央ペインにピン留め→最新ポストを連続表示する。
  // 広幅では中央にピン留めのみ、右ペインに最新ポストを時系列表示する。
  const right = !isNarrow ? recentSection : null;

  return (
    <>
      <AppShell
        center={
          <>
            {center}
            {pinnedSection}
            {isNarrow && recentSection}
          </>
        }
        right={right}
      />

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
