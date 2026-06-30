import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { api, UserProfile } from "../api/client";
import styles from "./UserProfile.module.css";

export default function UserProfilePage() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const q = searchParams.get("q") ?? "";

  const [profile, setProfile] = useState<UserProfile | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [following, setFollowing] = useState(false);
  const [followStatus, setFollowStatus] = useState<"not_following" | "pending" | "accepted">("not_following");

  useEffect(() => {
    if (!q) return;
    setLoading(true);
    setError("");
    api.users
      .profile(q)
      .then((p) => {
        setProfile(p);
        setFollowStatus(p.follow_status);
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, [q]);

  async function handleFollow() {
    if (!profile) return;
    setFollowing(true);
    try {
      const target = profile.ap_uri || `${profile.username}@${profile.domain}`;
      await api.follows.create(target);
      setFollowStatus("pending");
    } catch (e) {
      alert(e instanceof Error ? e.message : "フォローに失敗しました");
    } finally {
      setFollowing(false);
    }
  }

  function formatDate(iso: string) {
    return new Date(iso).toLocaleString("ja-JP", {
      month: "numeric",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  const isLocal = profile?.actor_type === "local";

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <button className={styles.backBtn} onClick={() => navigate(-1)}>
          ← 戻る
        </button>
        <span className={styles.logo}>seiran</span>
      </header>

      <main className={styles.main}>
        {loading && <p className={styles.message}>読み込み中...</p>}
        {error && <p className={styles.error}>{error}</p>}

        {profile && (
          <>
            <div className={styles.profileCard}>
              <div className={styles.profileNames}>
                <span className={styles.displayName}>
                  {profile.display_name || profile.username}
                </span>
                <span className={styles.acct}>
                  @{profile.username}@{profile.domain}
                </span>
              </div>

              {!isLocal && (
                <div className={styles.followArea}>
                  {followStatus === "accepted" && (
                    <span className={styles.followingBadge}>フォロー中</span>
                  )}
                  {followStatus === "pending" && (
                    <span className={styles.pendingBadge}>承認待ち</span>
                  )}
                  {followStatus === "not_following" && (
                    <button
                      className={styles.followBtn}
                      onClick={handleFollow}
                      disabled={following}
                    >
                      {following ? "送信中..." : "フォロー"}
                    </button>
                  )}
                </div>
              )}
            </div>

            <section className={styles.posts}>
              <h2 className={styles.postsTitle}>投稿</h2>
              {profile.recent_posts.length === 0 ? (
                <p className={styles.message}>投稿がありません</p>
              ) : (
                profile.recent_posts.map((post) => (
                  <article key={post.id} className={styles.post}>
                    <p className={styles.postBody}>{post.text}</p>
                    <time className={styles.postTime}>{formatDate(post.created_at)}</time>
                  </article>
                ))
              )}
            </section>
          </>
        )}
      </main>
    </div>
  );
}
