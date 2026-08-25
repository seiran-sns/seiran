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

/** `card.embedSrc`（oEmbed discoveryで解決済み、バックエンド側でホワイトリスト判定済み）の
 * ホストから、CSS aspect比（video/audio）を決める最小限のマッピング。個別サービスの
 * URL解析（動画ID抽出等）はバックエンド側のoEmbed discoveryに一本化されており、ここでは
 * 行わない。未知ホスト（管理者がホワイトリストに追加した将来のサービス等）はvideo扱いに
 * フォールバックする。 */
function embedAspectOf(embedSrc: string): "video" | "audio" {
  const host = hostnameOf(embedSrc);
  const audioHosts = ["open.spotify.com", "w.soundcloud.com", "embed.music.apple.com"];
  return audioHosts.some((h) => host === h || host.endsWith(`.${h}`)) ? "audio" : "video";
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

/** 一般URLカード（埋め込みプレーヤー・x.com以外）。 */
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

/** oEmbed discoveryで解決された埋め込みプレーヤー（YouTube/Spotify/Apple Music/SoundCloud/
 * Vimeo等、管理者ホワイトリストで許可されたドメインのみ）共通: クリックするまではサムネイル+
 * 再生ボタンのみ表示し、クリックした時点で初めて公式iframeプレイヤーを読み込む
 * （プライバシー・パフォーマンス配慮）。`sandbox`はトップレベルナビゲーションを禁止しつつ
 * 埋め込みプレーヤーの動作に必要な最小限の権限を許可する（Apple Music公式oEmbedの
 * sandbox値を参考）。 */
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
          sandbox="allow-forms allow-popups allow-same-origin allow-scripts allow-storage-access-by-user-activation allow-top-navigation-by-user-activation"
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

/** 投稿に添付されたURLカード（oEmbed埋め込みプレーヤー/x.com/一般の3種）の表示。
 * 埋め込みプレーヤーの対象サービス（YouTube/Spotify/Apple Music/SoundCloud/Vimeo等）は
 * バックエンド側のoEmbed discovery＋管理者ホワイトリスト判定で決まり、フロントは
 * `card.embedSrc`の有無だけで振り分ける（個別サービスのURL解析は行わない）。 */
export default function LinkCard({ card, indent = true }: LinkCardProps) {
  const { t } = useTranslation();
  const host = hostnameOf(card.url);

  const content = (() => {
    if (card.embedSrc) {
      return (
        <EmbedPlayerCard
          card={card}
          embedSrc={card.embedSrc}
          aspect={embedAspectOf(card.embedSrc)}
          label={t("home:noteCard.linkCardLoadEmbed")}
        />
      );
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
