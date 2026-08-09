import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { LinkCard as LinkCardData } from "../../api/client";
import { mediaUrl } from "../../utils/mediaProxy";
import styles from "./LinkCard.module.css";

interface LinkCardProps {
  card: LinkCardData;
  /** 顔アイコン分の左インデント（本文・添付画像・引用ブロック等と同じ50px）を付けるか。
   * 通常カードでは必要、既にインデント済みのQuoteCard内ではfalseを渡す。 */
  indent?: boolean;
}

interface TwitterWidgets {
  widgets?: {
    load: (el?: HTMLElement) => void;
  };
}

declare global {
  interface Window {
    twttr?: TwitterWidgets;
  }
}

function hostnameOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}

/** YouTube の動画ID（`v=`/`youtu.be/ID`/`/shorts/ID`/`/embed/ID`）を抽出する。 */
function extractYouTubeVideoId(url: string): string | null {
  try {
    const u = new URL(url);
    const host = u.hostname.replace(/^www\./, "").replace(/^m\./, "");
    if (host === "youtu.be") {
      return u.pathname.slice(1).split("/")[0] || null;
    }
    if (host === "youtube.com" || host === "music.youtube.com") {
      const v = u.searchParams.get("v");
      if (v) return v;
      const shorts = u.pathname.match(/^\/shorts\/([^/]+)/);
      if (shorts) return shorts[1];
      const embed = u.pathname.match(/^\/embed\/([^/]+)/);
      if (embed) return embed[1];
    }
  } catch {
    // 不正なURLはgenericカードにフォールバック
  }
  return null;
}

/** Spotify の embed パス（`track/ID` 等）を抽出する。地域プレフィックス（`/intl-ja/track/ID` 等）も許容する。 */
function extractSpotifyEmbedPath(url: string): string | null {
  try {
    const u = new URL(url);
    const match = u.pathname.match(
      /^\/(?:intl-[a-z]{2}\/)?(track|album|playlist|episode|show|artist)\/([A-Za-z0-9]+)/,
    );
    if (match) return `${match[1]}/${match[2]}`;
  } catch {
    // 不正なURLはgenericカードにフォールバック
  }
  return null;
}

function extractTweetId(url: string): string | null {
  try {
    const match = new URL(url).pathname.match(/\/status\/(\d+)/);
    return match ? match[1] : null;
  } catch {
    return null;
  }
}

let twitterWidgetsPromise: Promise<void> | null = null;
/** `platform.twitter.com/widgets.js` を一度だけ読み込む（複数カードでの重複読み込みを避ける）。 */
function loadTwitterWidgets(): Promise<void> {
  if (window.twttr?.widgets) return Promise.resolve();
  if (twitterWidgetsPromise) return twitterWidgetsPromise;
  twitterWidgetsPromise = new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = "https://platform.twitter.com/widgets.js";
    script.async = true;
    script.onload = () => resolve();
    script.onerror = () => reject(new Error("twitter widgets.js load failed"));
    document.body.appendChild(script);
  });
  return twitterWidgetsPromise;
}

/** 一般URLカード（YouTube/Spotify/x.com以外、またはIDが抽出できなかった場合）。 */
function GenericCard({ card }: { card: LinkCardData }) {
  return (
    <a
      href={card.url}
      target="_blank"
      rel="noopener noreferrer"
      className={styles.genericCard}
      onClick={(e) => e.stopPropagation()}
    >
      {card.thumbnailUrl && (
        <img
          src={mediaUrl(card.thumbnailUrl)}
          alt=""
          className={styles.genericThumb}
          loading="lazy"
        />
      )}
      <div className={styles.genericBody}>
        {card.title && <div className={styles.genericTitle}>{card.title}</div>}
        {card.description && (
          <div className={styles.genericDescription}>{card.description}</div>
        )}
        <div className={styles.genericDomain}>{hostnameOf(card.url)}</div>
      </div>
    </a>
  );
}

/** YouTube/Spotify共通: クリックするまではサムネイル+再生ボタンのみ表示し、
 * クリックした時点で初めて公式iframeプレイヤーを読み込む（プライバシー・パフォーマンス配慮）。 */
function EmbedPlayerCard({
  card,
  embedSrc,
  aspect,
  label,
}: {
  card: LinkCardData;
  embedSrc: string;
  aspect: "video" | "audio";
  label: string;
}) {
  const [activated, setActivated] = useState(false);

  if (activated) {
    return (
      <div
        className={aspect === "video" ? styles.videoPlayerWrap : styles.audioPlayerWrap}
        onClick={(e) => e.stopPropagation()}
      >
        <iframe
          src={embedSrc}
          className={styles.playerFrame}
          allow="autoplay; encrypted-media; picture-in-picture; fullscreen"
          title={card.title || card.url}
        />
      </div>
    );
  }

  return (
    <button
      type="button"
      className={styles.playerThumbButton}
      aria-label={label}
      onClick={(e) => {
        e.stopPropagation();
        setActivated(true);
      }}
    >
      {card.thumbnailUrl ? (
        <img
          src={mediaUrl(card.thumbnailUrl)}
          alt=""
          className={styles.playerThumb}
          loading="lazy"
        />
      ) : (
        <div className={styles.playerThumbFallback} />
      )}
      <span className={styles.playButtonOverlay} aria-hidden="true">
        ▶
      </span>
      {card.title && <span className={styles.playerTitle}>{card.title}</span>}
    </button>
  );
}

/** x.com: クリックした時点で公式 widgets.js を読み込み、ツイート本文をライブ表示する。 */
function TwitterCard({ card, tweetId }: { card: LinkCardData; tweetId: string }) {
  const { t } = useTranslation();
  const [activated, setActivated] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!activated) return;
    loadTwitterWidgets().then(() => {
      if (containerRef.current) {
        window.twttr?.widgets?.load(containerRef.current);
      }
    });
  }, [activated]);

  if (!activated) {
    return (
      <button
        type="button"
        className={styles.genericCard}
        onClick={(e) => {
          e.stopPropagation();
          setActivated(true);
        }}
      >
        {card.thumbnailUrl && (
          <img
            src={mediaUrl(card.thumbnailUrl)}
            alt=""
            className={styles.genericThumb}
            loading="lazy"
          />
        )}
        <div className={styles.genericBody}>
          {card.title && <div className={styles.genericTitle}>{card.title}</div>}
          {card.description && (
            <div className={styles.genericDescription}>{card.description}</div>
          )}
          <div className={styles.genericDomain}>{t("home:noteCard.linkCardLoadEmbed")}</div>
        </div>
      </button>
    );
  }

  return (
    <div ref={containerRef} className={styles.tweetWrap} onClick={(e) => e.stopPropagation()}>
      <blockquote className="twitter-tweet">
        <a href={`https://twitter.com/i/status/${tweetId}`}>{card.url}</a>
      </blockquote>
    </div>
  );
}

/** 投稿に添付されたURLカード（YouTube/Spotify/x.com/一般の4種）の表示。 */
export default function LinkCard({ card, indent = true }: LinkCardProps) {
  const { t } = useTranslation();
  const host = hostnameOf(card.url);

  const content = (() => {
    if (host === "youtube.com" || host === "youtu.be" || host === "music.youtube.com") {
      const videoId = extractYouTubeVideoId(card.url);
      if (videoId) {
        return (
          <EmbedPlayerCard
            card={card}
            embedSrc={`https://www.youtube-nocookie.com/embed/${videoId}?autoplay=1`}
            aspect="video"
            label={t("home:noteCard.linkCardLoadEmbed")}
          />
        );
      }
    }

    if (host === "open.spotify.com") {
      const embedPath = extractSpotifyEmbedPath(card.url);
      if (embedPath) {
        return (
          <EmbedPlayerCard
            card={card}
            embedSrc={`https://open.spotify.com/embed/${embedPath}`}
            aspect="audio"
            label={t("home:noteCard.linkCardLoadEmbed")}
          />
        );
      }
    }

    if (host === "x.com" || host === "twitter.com") {
      const tweetId = extractTweetId(card.url);
      if (tweetId) {
        return <TwitterCard card={card} tweetId={tweetId} />;
      }
    }

    return <GenericCard card={card} />;
  })();

  return indent ? <div className={styles.indentWrap}>{content}</div> : content;
}
