import { NoteAttachment } from "../../api/client";
import HlsVideo from "./HlsVideo";
import styles from "./NoteCard.module.css";

interface NoteAttachmentsProps {
  attachments?: NoteAttachment[];
}

/** 投稿に添付されたメディア（画像/動画/HLS/音声）一覧の表示。 */
export default function NoteAttachments({ attachments }: NoteAttachmentsProps) {
  if (!attachments || attachments.length === 0) return null;

  return (
    <div className={styles.attachments}>
      {attachments.map((att, i) => {
        const isHls = att.mimeType === "application/vnd.apple.mpegurl" || att.mimeType === "application/x-mpegURL";
        if (att.mimeType.startsWith("video/") || isHls) {
          return (
            <HlsVideo
              key={i}
              src={att.url}
              poster={att.thumbnailUrl}
              isHls={isHls}
              className={styles.attachImage}
              onClick={(e) => e.stopPropagation()}
            />
          );
        }
        if (att.mimeType.startsWith("audio/")) {
          return (
            <audio
              key={i}
              src={att.url}
              controls
              className={styles.attachAudio}
              onClick={(e) => e.stopPropagation()}
            />
          );
        }
        return (
          <a
            key={i}
            href={att.url}
            target="_blank"
            rel="noopener noreferrer"
            onClick={(e) => e.stopPropagation()}
          >
            <img src={att.url} alt="" className={styles.attachImage} loading="lazy" />
          </a>
        );
      })}
    </div>
  );
}
