import { Trans } from "react-i18next";
import { Link, useNavigate } from "react-router-dom";
import i18n from "../../i18n";
import { ReactionEvent } from "../../api/client";
import { profilePath } from "../../lib/format";
import { mediaUrl } from "../../utils/mediaProxy";
import Avatar from "./Avatar";
import EmojiText from "./EmojiText";
import NoteHoverPreview from "./NoteHoverPreview";
import TwemojiEmoji from "../common/TwemojiEmoji";
import styles from "./ReactionEventCard.module.css";

interface ReactionEventCardProps {
  event: ReactionEvent;
}

/**
 * プロフィール「投稿」タブの投稿＋リアクションイベント混合表示における1件のリアクション
 * イベント。クイック通知タブ（`NotificationsPanel`）のカードと同じ体裁（角丸カード・
 * クリックで対象ポストへ遷移）を使うが、中身は絵文字・アバター・文面をインライン要素として
 * 半角スペース区切りで並べる単純な1行構成にする（`flex`行での構築はNoteHoverPreviewの
 * ラッパー要素との組み合わせでCSS読み込み順に依存する不具合があったため採用しない）。
 * このタブは表示対象＝リアクションした人がプロフィール本人に確定しているため、通知パネルと
 * 違い文面に「誰が」は含めない。
 */
export default function ReactionEventCard({ event }: ReactionEventCardProps) {
  const navigate = useNavigate();

  const who = event.targetUser.displayName || event.targetUser.username;
  const userLink = (
    <Link
      to={profilePath(event.targetUser.username, event.targetUser.domain)}
      className={styles.userLink}
      onClick={(e) => e.stopPropagation()}
    />
  );
  const emojiName = <EmojiText text={who} emojis={event.targetUserEmojis} />;

  return (
    <div className={styles.item} onClick={() => navigate(`/notes/${event.targetNoteId}`)}>
      <NoteHoverPreview noteId={event.targetNoteId} className={styles.previewWrap}>
        {event.reactionEmojiUrl ? (
          <img
            className={styles.iconImg}
            src={mediaUrl(event.reactionEmojiUrl)}
            alt={event.reaction}
            title={event.reaction}
            loading="lazy"
          />
        ) : (
          <TwemojiEmoji emoji={event.reaction} className={styles.icon} />
        )}{" "}
        <span className={styles.avatarWrap}>
          <Avatar url={event.targetUser.avatarUrl} name={who} size={20} />
        </span>{" "}
        <span className={styles.text}>
          <Trans
            i18n={i18n}
            i18nKey="profile:profilePage.reactionEventText"
            components={{ userLink, emojiName }}
          />
        </span>
      </NoteHoverPreview>
    </div>
  );
}
