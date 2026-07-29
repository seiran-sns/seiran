import { useState } from "react";
import { useTranslation } from "react-i18next";
import { NoteAttachment } from "../../api/client";
import ImageLightbox from "../common/ImageLightbox";
import HlsVideo from "./HlsVideo";
import styles from "./NoteCard.module.css";
import { mediaUrl } from "../../utils/mediaProxy";

interface NoteAttachmentsProps {
  attachments?: NoteAttachment[];
}

function isImage(att: NoteAttachment) {
  const isHls =
    att.mimeType === "application/vnd.apple.mpegurl" ||
    att.mimeType === "application/x-mpegURL";
  return !att.mimeType.startsWith("video/") && !att.mimeType.startsWith("audio/") && !isHls;
}

/** 投稿に添付されたメディア（画像/動画/HLS/音声）一覧の表示。 */
export default function NoteAttachments({ attachments }: NoteAttachmentsProps) {
  const { t } = useTranslation();
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);
  const [revealed, setRevealed] = useState<Set<number>>(() => new Set());

  if (!attachments || attachments.length === 0) return null;

  const images = attachments.filter(isImage);

  return (
    <div className={styles.attachments}>
      {attachments.map((att, i) => {
        const isHls =
          att.mimeType === "application/vnd.apple.mpegurl" ||
          att.mimeType === "application/x-mpegURL";
        if (att.mimeType.startsWith("video/") || isHls) {
          return (
            <HlsVideo
              key={i}
              src={mediaUrl(att.url) ?? att.url}
              poster={mediaUrl(att.thumbnailUrl)}
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
              src={mediaUrl(att.url)}
              controls
              className={styles.attachAudio}
              onClick={(e) => e.stopPropagation()}
            />
          );
        }

        const isRevealed = revealed.has(i);
        const imageIndex = images.indexOf(att);
        return (
          <div key={i} className={styles.sensitiveImageWrap}>
            <img
              src={mediaUrl(att.url)}
              alt=""
              className={`${styles.attachImage} ${
                att.isSensitive && !isRevealed ? styles.attachBlurred : ""
              }`}
              loading="lazy"
              onClick={(e) => {
                e.stopPropagation();
                if (att.isSensitive && isRevealed) {
                  setRevealed((current) => {
                    const next = new Set(current);
                    next.delete(i);
                    return next;
                  });
                } else {
                  setLightboxIndex(imageIndex);
                }
              }}
            />
            {att.isSensitive && !isRevealed && (
              <button
                className={styles.sensitiveReveal}
                aria-label={t("common:sensitiveImageReveal")}
                onClick={(e) => {
                  e.stopPropagation();
                  setRevealed((current) => new Set(current).add(i));
                }}
              >
                👁️
              </button>
            )}
          </div>
        );
      })}
      <ImageLightbox
        src={
          lightboxIndex === null
            ? null
            : (mediaUrl(images[lightboxIndex]?.url) ?? images[lightboxIndex]?.url ?? null)
        }
        sensitive={lightboxIndex === null ? false : images[lightboxIndex]?.isSensitive}
        onClose={() => setLightboxIndex(null)}
        onPrevious={
          lightboxIndex !== null && lightboxIndex > 0
            ? () => setLightboxIndex(lightboxIndex - 1)
            : undefined
        }
        onNext={
          lightboxIndex !== null && lightboxIndex < images.length - 1
            ? () => setLightboxIndex(lightboxIndex + 1)
            : undefined
        }
      />
    </div>
  );
}
