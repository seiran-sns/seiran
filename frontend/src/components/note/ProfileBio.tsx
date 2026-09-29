import { ResolvedLinkInfo, UserProfile } from "../../api/types";
import { toProfileHtml } from "../../lib/profileHtml";
import RichHtml from "./RichHtml";

interface ProfileBioProps {
  profile: Pick<UserProfile, "bio" | "actor_type" | "emojis">;
  resolvedLinks: Record<string, ResolvedLinkInfo>;
  className?: string;
}

/** プロフィールのbio表示。プロフィールページと投稿者パネルで共通（サニタイズ済みHTML/プレーンテキストの分岐・リンク解決込み）。 */
export default function ProfileBio({ profile, resolvedLinks, className }: ProfileBioProps) {
  if (!profile.bio) return null;
  return (
    <p className={className}>
      <RichHtml
        html={toProfileHtml(profile.bio, profile.actor_type)}
        emojis={profile.emojis}
        resolvedLinks={resolvedLinks}
      />
    </p>
  );
}
