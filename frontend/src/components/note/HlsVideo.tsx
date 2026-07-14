import { useEffect, useRef } from "react";
import Hls from "hls.js";

interface HlsVideoProps {
  src: string;
  poster?: string;
  className?: string;
  /** true の場合 src は HLS(.m3u8)プレイリスト。Safari はネイティブ再生、
   * それ以外(Chrome/Firefox等)は hls.js 経由で再生する。 */
  isHls?: boolean;
  onClick?: (e: React.MouseEvent) => void;
}

/**
 * Bsky発の動画添付はBluesky公式の動画パイプラインでHLSにトランスコードされて
 * 配信される（`.m3u8`）。ネイティブHLS再生はSafariのみ対応のため、それ以外の
 * ブラウザでは hls.js でMediaSource Extensions経由で再生する。
 */
export default function HlsVideo({ src, poster, className, isHls, onClick }: HlsVideoProps) {
  const videoRef = useRef<HTMLVideoElement>(null);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !isHls) return;

    if (video.canPlayType("application/vnd.apple.mpegurl")) {
      video.src = src;
      return;
    }
    if (Hls.isSupported()) {
      const hls = new Hls();
      hls.loadSource(src);
      hls.attachMedia(video);
      return () => hls.destroy();
    }
  }, [src, isHls]);

  return (
    <video
      ref={videoRef}
      src={isHls ? undefined : src}
      poster={poster}
      controls
      className={className}
      onClick={onClick}
    />
  );
}
