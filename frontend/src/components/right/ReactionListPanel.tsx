import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, Note, ReactionActor } from "../../api/client";
import { mediaUrl } from "../../utils/mediaProxy";
import Avatar from "../note/Avatar";
import EmojiContextMenu from "../note/EmojiContextMenu";
import { parseCustomEmojiShortcode } from "../../lib/customEmojis";
import TwemojiEmoji from "../common/TwemojiEmoji";
import panel from "../common/Panel.module.css";
import styles from "./ReactionListPanel.module.css";

interface ReactionListPanelProps {
  note: Note;
}

type ActorsState = ReactionActor[] | "loading" | "error";

/** ポスト詳細右ペインの「リアクション」タブ（#226）: 全リアクション×付けたユーザー一覧を
 * 絵文字ごとにグループ化して表示する（`ReactionChips`のホバーポップオーバーと異なり、
 * ホバー不要で全件を常時展開表示する）。 */
export default function ReactionListPanel({ note }: ReactionListPanelProps) {
  const { t } = useTranslation();
  const reactions = note.reactions ?? [];
  const [actorsByEmoji, setActorsByEmoji] = useState<Record<string, ActorsState>>({});

  useEffect(() => {
    let cancelled = false;
    const initial: Record<string, ActorsState> = {};
    for (const r of reactions) initial[r.emoji] = "loading";
    setActorsByEmoji(initial);

    reactions.forEach((r) => {
      api.notes
        .reactionActors(note.id, r.emoji)
        .then((res) => {
          if (!cancelled) setActorsByEmoji((prev) => ({ ...prev, [r.emoji]: res.actors }));
        })
        .catch(() => {
          if (!cancelled) setActorsByEmoji((prev) => ({ ...prev, [r.emoji]: "error" }));
        });
    });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [note.id, reactions.map((r) => r.emoji).join(",")]);

  if (reactions.length === 0) {
    return (
      <div className={panel.placeholder}>
        <TwemojiEmoji emoji="😀" className={panel.placeholderIcon} />
        {t("home:noteDetailPage.noReactions")}
      </div>
    );
  }

  return (
    <div>
      {reactions.map((r) => {
        const shortcode = parseCustomEmojiShortcode(r.emoji);
        const actors = actorsByEmoji[r.emoji];
        return (
          <div key={r.emoji} className={styles.group}>
            <div className={styles.groupHeader}>
              {r.emojiUrl && shortcode ? (
                <EmojiContextMenu shortcode={shortcode} imageUrl={r.emojiUrl}>
                  <img className={styles.emojiImg} src={mediaUrl(r.emojiUrl)} alt={r.emoji} loading="lazy" />
                </EmojiContextMenu>
              ) : r.emojiUrl ? (
                <img className={styles.emojiImg} src={mediaUrl(r.emojiUrl)} alt={r.emoji} loading="lazy" />
              ) : (
                <TwemojiEmoji emoji={r.emoji} className={styles.emojiImg} />
              )}
              <span className={styles.count}>{r.count}</span>
            </div>
            {actors === "loading" && <p className={panel.message}>{t("common:loading")}</p>}
            {actors === "error" && (
              <p className={panel.message}>{t("home:reactionChips.actorsFetchFailed")}</p>
            )}
            {Array.isArray(actors) && (
              <ul className={styles.actorList}>
                {actors.map((a) => (
                  <li key={a.id} className={styles.actorRow}>
                    <Avatar url={a.avatarUrl} name={a.displayName || a.username} size={24} />
                    <span className={styles.actorName}>{a.displayName || a.username}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        );
      })}
    </div>
  );
}
