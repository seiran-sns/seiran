import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { api, Note, UserProfile, getErrorMessage } from "../../api/client";
import { profilePath, profileQuery } from "../../lib/format";
import Avatar from "../note/Avatar";
import EmojiText from "../note/EmojiText";
import NoteCard from "../note/NoteCard";
import ProfileBio from "../note/ProfileBio";
import { useResolvedLinks } from "../../hooks/useResolvedLinks";
import panel from "../common/Panel.module.css";
import styles from "./AuthorPanel.module.css";

interface AuthorPanelProps {
  /** 表示対象のポスト。リポストの場合は呼び出し側でリポスト元実体を渡すこと。 */
  note: Note;
}

/** ポスト詳細右ペインの「投稿者」タブ（#226）: プロフィール概要と固定ポストを表示する。 */
export default function AuthorPanel({ note }: AuthorPanelProps) {
  const { t } = useTranslation();
  const [profile, setProfile] = useState<UserProfile | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [resolvedLinks, setResolvedLinks] = useResolvedLinks(undefined);

  const q = profileQuery(note.user.username, note.user.domain);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError("");
    api.users
      .profile(q)
      .then((p) => {
        if (cancelled) return;
        setProfile(p);
        setResolvedLinks(p.link_resolutions ?? {});
      })
      .catch((e) => !cancelled && setError(getErrorMessage(e)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q]);

  if (loading) return <p className={panel.message}>{t("common:loading")}</p>;
  if (error) return <p className={panel.message}>{error}</p>;
  if (!profile) return null;

  return (
    <div>
      <Link to={profilePath(profile.username, profile.domain)} className={styles.card}>
        <Avatar
          url={profile.avatar_url}
          name={profile.display_name || profile.username}
          size={56}
        />
        <div className={styles.names}>
          <span className={styles.displayName}>
            <EmojiText text={profile.display_name || profile.username} emojis={profile.emojis} />
          </span>
          <span className={styles.acct}>
            @{profile.username}
            {profile.domain && profile.domain !== window.location.hostname && `@${profile.domain}`}
          </span>
        </div>
      </Link>

      <ProfileBio profile={profile} resolvedLinks={resolvedLinks} className={styles.bio} />

      {profile.actor_id && (
        <div className={styles.counts}>
          <Link to={profilePath(profile.username, profile.domain)} className={styles.countItem}>
            <strong>{profile.following_count}</strong>{" "}
            {t("profile:profilePage.followingCountLabel")}
          </Link>
          <Link to={profilePath(profile.username, profile.domain)} className={styles.countItem}>
            <strong>{profile.follower_count}</strong>{" "}
            {t("profile:profilePage.followerCountLabel")}
          </Link>
        </div>
      )}

      {profile.pinned_posts.length > 0 ? (
        <>
          <div className={panel.rightHeader}>{t("profile:profilePage.pinnedHeader")}</div>
          {profile.pinned_posts.map((post) => (
            <NoteCard key={post.id} note={post} />
          ))}
        </>
      ) : (
        <p className={panel.message}>{t("home:noteDetailPage.noPinnedPosts")}</p>
      )}
    </div>
  );
}
