import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Note } from "../../api/client";
import EmojiText from "./EmojiText";
import RichHtml from "./RichHtml";
import RichText from "./RichText";
import NoteAttachments from "./NoteAttachments";
import LinkCard from "./LinkCard";
import TwemojiEmoji from "../common/TwemojiEmoji";
import { QuoteCard } from "./NoteCard";
import styles from "./MessageContent.module.css";

/** メッセージ画面（DM）のメッセージ本文表示。NoteCardの本文表示ロジック（CW開閉・本文・
 * 添付・リンクカード・アンケート・引用）のうち、投稿者ヘッダー等メッセージバブルに
 * 不要な部分を除いた軽量版。アンケートは結果表示のみで投票はできない
 * （`QuoteCard`の引用先アンケート表示と同じ扱い）。 */
export default function MessageContent({ note }: { note: Note }) {
  const { t } = useTranslation();
  const [showContent, setShowContent] = useState(!note.contentWarning);

  return (
    <>
      {note.contentWarning && (
        <div className={styles.contentWarningWrap}>
          <span>
            <TwemojiEmoji emoji="⚠️" /> <EmojiText text={note.contentWarning} emojis={note.emojis} />
          </span>
          <button type="button" onClick={() => setShowContent((shown) => !shown)}>
            {showContent ? t("home:noteCard.hideContent") : t("home:noteCard.showContent")}
          </button>
        </div>
      )}
      {showContent && (
        <>
          {note.text && (
            <p className={styles.body}>
              {note.contentHtml ? (
                <RichHtml html={note.contentHtml} emojis={note.emojis} />
              ) : (
                <RichText text={note.text} emojis={note.emojis} />
              )}
            </p>
          )}
          <NoteAttachments attachments={note.attachments} />
          {note.linkCards.map((card) => (
            <LinkCard key={card.url} card={card} indent={false} />
          ))}
          {note.poll && (
            <div className={styles.poll}>
              {note.poll.options.map((option) => (
                <div className={styles.pollOption} key={option.name}>
                  <span>{option.name}</span>
                  <span>{t("home:noteCard.votes", { count: option.votes })}</span>
                </div>
              ))}
            </div>
          )}
          {note.quote && <QuoteCard note={note.quote} />}
        </>
      )}
    </>
  );
}
