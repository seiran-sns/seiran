import { useCallback, useEffect, useRef, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, Note, UserProfile, getErrorMessage } from "../api/client";
import ActionsMenu, { ActionsMenuItem } from "../components/common/ActionsMenu";
import Modal from "../components/common/Modal";
import RemoteBanner from "../components/common/RemoteBanner";
import AppShell from "../components/layout/AppShell";
import NoteCard from "../components/note/NoteCard";
import NoteList from "../components/note/NoteList";
import { useAuth } from "../contexts/AuthContext";
import { useGoBack } from "../contexts/NavigationHistoryContext";
import { useToast } from "../contexts/ToastContext";
import { useCursorPagination } from "../hooks/useCursorPagination";
import { useIsNarrowViewport } from "../hooks/useIsNarrowViewport";
import { profileQuery, remoteProfileUrl } from "../lib/format";
import { setFollowStatus as setFollowStatusStore, useFollowStatus } from "../stores/followStatusStore";
import panel from "../components/common/Panel.module.css";
import styles from "./ProfilePage.module.css";

const PAGE_SIZE = 20;

export default function ProfilePage() {
  const { t } = useTranslation();
  const { showError } = useToast();
  const [searchParams] = useSearchParams();
  const { acct } = useParams<{ acct: string }>();
  const navigate = useNavigate();
  const goBack = useGoBack();
  // permalink `/@handle`（#36）を優先し、旧 `/profile?q=` も後方互換で受ける。
  const q = acct ? acct.replace(/^@/, "") : searchParams.get("q") ?? "";

  const [profile, setProfile] = useState<UserProfile | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [following, setFollowing] = useState(false);
  const [unfollowing, setUnfollowing] = useState(false);
  const [bridgeModalOpen, setBridgeModalOpen] = useState(false);
  const [isBlocking, setIsBlocking] = useState(false);
  const [isBlockedBy, setIsBlockedBy] = useState(false);
  const [isMuted, setIsMuted] = useState(false);
  const [blockActionLoading, setBlockActionLoading] = useState(false);
  const [muteActionLoading, setMuteActionLoading] = useState(false);
  const [blockConfirmModalOpen, setBlockConfirmModalOpen] = useState(false);
  // 狭幅では右ペインが無いため、ピン留め・最新ポストの両方を中央ペインへ連続表示する（#61）。
  const isNarrow = useIsNarrowViewport();

  // 投稿一覧の無限スクロール（#64）。`profile.recent_posts`（初回最大20件）を初期値とし、
  // 下端到達で `GET /api/users/posts` から `until_id` カーソルで追加取得する。
  const actorIdRef = useRef<string | undefined>(undefined);

  const onError = useCallback((e: unknown) => showError(getErrorMessage(e)), [showError]);
  const fetchPage = useCallback((untilId: string) => {
    // actorIdRef は profile 取得完了後にのみ設定され、hasMore も同時に true になるため、
    // loadMore が呼ばれる時点では必ず値が入っている。
    return api.users.posts(actorIdRef.current as string, { limit: PAGE_SIZE, until_id: untilId, exclude_direct: true });
  }, []);
  const { items: posts, setItems: setPosts, hasMore, setHasMore, loadingMore, loadMore: loadMorePosts } = useCursorPagination<Note>(
    fetchPage,
    (n) => n.id,
    PAGE_SIZE,
    onError
  );

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
        setFollowStatusStore(profileQuery(p.username, p.domain), p.follow_status);
        setIsBlocking(p.is_blocking);
        setIsBlockedBy(p.is_blocked_by);
        setIsMuted(p.is_muted);
        actorIdRef.current = p.actor_id;
        setPosts(p.recent_posts);
        setHasMore(!!p.actor_id && p.recent_posts.length >= PAGE_SIZE);
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q]);

  const { user } = useAuth();

  // フォロー状態は共有ストア（stores/followStatusStore）から取得する。プロフィール本体・
  // 右ペインのポストリスト・タイムライン上の同一ユーザーのフォロースイッチが全て同じキーを
  // 参照するため、いずれかで操作するか WebSocket の `followAccepted`（StreamingContext）を
  // 受けるだけで、表示中の全コンポーネントに同時反映される。
  const followKey = profile ? profileQuery(profile.username, profile.domain) : "";
  const followStatus = useFollowStatus(followKey) ?? "not_following";

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
      setFollowStatusStore(followKey, result.status === "accepted" ? "accepted" : "pending");
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setFollowing(false);
    }
  }

  async function doUnfollow() {
    if (!profile) return;
    setUnfollowing(true);
    try {
      await api.follows.delete(followTarget());
      setFollowStatusStore(followKey, "not_following");
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setUnfollowing(false);
    }
  }

  // ブロック・ミュートは相手のプロフィールに表示中の投稿一覧（recent_posts）自体の
  // 表示可否も変える（actor_is_hidden_for_viewer によるタイムラインフィルタ）ため、
  // ローカルの状態フラグ更新だけでなくプロフィール全体を取り直して反映する。
  async function refreshProfile() {
    if (!q) return;
    try {
      const p = await api.users.profile(q);
      setProfile(p);
      setFollowStatusStore(profileQuery(p.username, p.domain), p.follow_status);
      setIsBlocking(p.is_blocking);
      setIsBlockedBy(p.is_blocked_by);
      setIsMuted(p.is_muted);
      setPosts(p.recent_posts);
      setHasMore(!!p.actor_id && p.recent_posts.length >= PAGE_SIZE);
    } catch {
      // ベストエフォート（ブロック/ミュート操作自体は既に成功しているため、再取得失敗はエラー表示しない）
    }
  }

  async function doMute() {
    if (!profile) return;
    setMuteActionLoading(true);
    try {
      await api.mutes.create(followTarget());
      await refreshProfile();
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setMuteActionLoading(false);
    }
  }

  async function doUnmute() {
    if (!profile) return;
    setMuteActionLoading(true);
    try {
      await api.mutes.delete(followTarget());
      await refreshProfile();
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setMuteActionLoading(false);
    }
  }

  // ブロックは破壊的操作（双方向フォロー強制解除を伴う）のため、確認モーダルを経由してから実行する。
  async function doBlock() {
    if (!profile) return;
    setBlockActionLoading(true);
    try {
      await api.blocks.create(followTarget());
      await refreshProfile();
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setBlockActionLoading(false);
      setBlockConfirmModalOpen(false);
    }
  }

  async function doUnblock() {
    if (!profile) return;
    setBlockActionLoading(true);
    try {
      await api.blocks.delete(followTarget());
      await refreshProfile();
    } catch (e) {
      showError(getErrorMessage(e));
    } finally {
      setBlockActionLoading(false);
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
        <button className={panel.backBtn} onClick={goBack}>
          ← {t("common:back")}
        </button>
        <span className={panel.title}>{t("profile:profilePage.title")}</span>
      </header>

      {profile && remoteProfileUrl(profile) && (
        <RemoteBanner message={t("common:remoteBanner.user")} url={remoteProfileUrl(profile) as string} />
      )}

      {loading && <p className={panel.message}>{t("common:loading")}</p>}
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
                {t("profile:profilePage.warpBanner.prefix")}
                <strong>{t("profile:profilePage.warpBanner.shadowLabel")}</strong>
                {t("profile:profilePage.warpBanner.suffix", { handle: profile.bridge_real_handle })}
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
              <span className={`${styles.badge} ${styles.pairedBadge}`} title={t("profile:profilePage.pairedBadgeTitle")}>
                🀄 {t("profile:profilePage.pairedBadge")}
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

          {/* プロフィールのキーバリュー項目（#62） */}
          {profile.profile_fields.length > 0 && (
            <div className={styles.identity}>
              {profile.profile_fields.map((field, i) => (
                <div className={styles.idRow} key={i}>
                  <span className={styles.idLabel}>{field.name}</span>
                  {field.value.startsWith("http://") || field.value.startsWith("https://") ? (
                    <a className={styles.idValue} href={field.value} target="_blank" rel="noopener noreferrer">
                      {field.value}
                    </a>
                  ) : (
                    <span className={styles.idValue}>{field.value}</span>
                  )}
                </div>
              ))}
            </div>
          )}

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
                {t("profile:profilePage.editProfileButton")}
              </button>
            </div>
          )}

          {!isSelf && (
            <div className={styles.followArea}>
              {followStatus === "accepted" && (
                <>
                  <span className={styles.followingBadge}>{t("profile:profilePage.followingBadge")}</span>
                  <button className={styles.unfollowBtn} onClick={doUnfollow} disabled={unfollowing}>
                    {unfollowing ? t("profile:profilePage.unfollowingButton") : t("profile:profilePage.unfollowButton")}
                  </button>
                </>
              )}
              {followStatus === "pending" && <span className={styles.pendingBadge}>{t("profile:profilePage.pendingBadge")}</span>}
              {followStatus === "not_following" && (
                <button className={styles.followBtn} onClick={handleFollowClick} disabled={following || isBlockedBy || isBlocking}>
                  {following ? t("profile:profilePage.followingSubmitButton") : t("profile:profilePage.followButton")}
                </button>
              )}
              {isBlockedBy && (
                <span className={styles.pendingBadge}>{t("profile:profilePage.blockedByNotice")}</span>
              )}
              <ActionsMenu
                triggerTitle={t("profile:profilePage.actionsMenuTitle")}
                items={(() => {
                  const items: ActionsMenuItem[] = [];
                  if (followStatus === "accepted") {
                    items.push({
                      key: "unfollow",
                      label: unfollowing ? t("profile:profilePage.unfollowingButton") : t("profile:profilePage.unfollowButton"),
                      onClick: doUnfollow,
                      disabled: unfollowing,
                    });
                  } else if (followStatus === "pending") {
                    items.push({
                      key: "pending",
                      label: t("profile:profilePage.pendingBadge"),
                      onClick: () => {},
                      disabled: true,
                    });
                  } else {
                    items.push({
                      key: "follow",
                      label: following ? t("profile:profilePage.followingSubmitButton") : t("profile:profilePage.followButton"),
                      onClick: handleFollowClick,
                      disabled: following || isBlockedBy || isBlocking,
                    });
                  }
                  items.push(
                    isMuted
                      ? { key: "unmute", label: t("profile:profilePage.unmuteButton"), onClick: doUnmute, disabled: muteActionLoading }
                      : { key: "mute", label: t("profile:profilePage.muteButton"), onClick: doMute, disabled: muteActionLoading }
                  );
                  items.push(
                    isBlocking
                      ? { key: "unblock", label: t("profile:profilePage.unblockButton"), onClick: doUnblock, danger: true, disabled: blockActionLoading }
                      : { key: "block", label: t("profile:profilePage.blockButton"), onClick: () => setBlockConfirmModalOpen(true), danger: true, disabled: blockActionLoading }
                  );
                  return items;
                })()}
              />
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
      <div className={panel.rightHeader}>{t("profile:profilePage.pinnedHeader")}</div>
      {profile.pinned_posts.map((post) => <NoteCard key={post.id} note={post} />)}
    </>
  );

  // 公開リスト一覧（#63）。現状ローカルユーザーのみ表示（リモートは将来課題）。
  const listsSection = profile && profile.public_lists.length > 0 && (
    <>
      <div className={panel.rightHeader}>{t("profile:profilePage.publicListsHeader")}</div>
      <div className={styles.listsRow}>
        {profile.public_lists.map((l) => (
          <Link key={l.id} to={`/lists/${l.id}`} className={styles.listBadge}>
            {l.name} <span className={styles.listBadgeCount}>{l.member_count}</span>
          </Link>
        ))}
      </div>
    </>
  );

  const recentSection = profile && (
    <>
      <div className={panel.rightHeader}>{t("profile:profilePage.postsHeader")}</div>
      <NoteList
        notes={posts}
        emptyMessage={t("profile:profilePage.noPosts")}
        onLoadMore={loadMorePosts}
        hasMore={hasMore}
        loadingMore={loadingMore}
      />
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
            {listsSection}
            {isNarrow && recentSection}
          </>
        }
        right={right}
      />

      <Modal
        open={bridgeModalOpen}
        onClose={() => setBridgeModalOpen(false)}
        title={t("profile:profilePage.bridgeModal.title")}
      >
        <p className={styles.modalText}>
          {t("profile:profilePage.bridgeModal.prefix", {
            protocol: profile?.bridge_protocol === "bsky" ? "Bluesky" : "Fediverse",
          })}
          <strong>{t("profile:profilePage.bridgeModal.shadowLabel")}</strong>
          {t("profile:profilePage.bridgeModal.suffix")}
        </p>
        <div className={styles.modalActions}>
          <button className={styles.modalPrimary} onClick={warpToReal}>
            {t("profile:profilePage.bridgeModal.goToRealButton")}
          </button>
          <button
            className={styles.modalSecondary}
            onClick={() => {
              setBridgeModalOpen(false);
              doFollow();
            }}
          >
            {t("profile:profilePage.bridgeModal.followAnywayButton")}
          </button>
        </div>
      </Modal>

      <Modal
        open={blockConfirmModalOpen}
        onClose={() => setBlockConfirmModalOpen(false)}
        title={t("profile:profilePage.blockConfirmModal.title")}
      >
        <p className={styles.modalText}>{t("profile:profilePage.blockConfirmModal.body")}</p>
        <div className={styles.modalActions}>
          <button className={styles.modalPrimary} onClick={doBlock} disabled={blockActionLoading}>
            {t("profile:profilePage.blockConfirmModal.confirmButton")}
          </button>
          <button className={styles.modalSecondary} onClick={() => setBlockConfirmModalOpen(false)}>
            {t("profile:profilePage.blockConfirmModal.cancelButton")}
          </button>
        </div>
      </Modal>
    </>
  );
}
