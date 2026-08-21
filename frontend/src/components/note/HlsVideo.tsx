import { useEffect, useRef } from "react";
import Hls from "hls.js";

interface HlsVideoProps {
  src: string;
  /** メディアプロキシを経由しない直URL。src(プロキシ経由)と異なる場合のみ、
   * <source>を2本並べてプロキシ→直の順にブラウザへフォールバックさせる
   * （プロキシのサイズ上限等で失敗した際に直アクセスへ自動的に切り替わる）。 */
  fallbackSrc?: string;
  mimeType?: string;
  poster?: string;
  className?: string;
  /** true の場合 src は HLS(.m3u8)プレイリスト。Safari はネイティブ再生、
   * それ以外(Chrome/Firefox等)は hls.js 経由で再生する。 */
  isHls?: boolean;
  /** true の場合GIFアニメ（Tenor/Klipy由来、またはBskyのGIF直接アップロード由来
   * `presentation:"gif"`）として扱い、自動再生・ミュート・ループ・コントロール無しで表示する。 */
  isGif?: boolean;
  onClick?: (e: React.MouseEvent) => void;
}

/**
 * Bsky発の動画添付はBluesky公式の動画パイプラインでHLSにトランスコードされて
 * 配信される（`.m3u8`）。`video.canPlayType("application/vnd.apple.mpegurl")`は
 * 仕様上「型として認識できるかもしれない」程度の判定で、実際にデコードできるかは
 * 保証しない。一部のChrome環境で`"maybe"`を誤って返し、ネイティブHLS対応と誤判定
 * されてhls.jsが一切初期化されず、`video.src`をセットするだけで実際には全く
 * 再生できない（データはダウンロードされるが再生可能状態にならない）不具合が
 * 実機で確認された。`Hls.isSupported()`（Media Source Extensionsの実サポートを
 * 正確にチェックする）を優先し、それが使えない環境（iOS Safari等）でのみ
 * ネイティブHLSにフォールバックする。
 */
export default function HlsVideo({
  src,
  fallbackSrc,
  mimeType,
  poster,
  className,
  isHls,
  isGif,
  onClick,
}: HlsVideoProps) {
  const videoRef = useRef<HTMLVideoElement>(null);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !isHls) return;

    if (Hls.isSupported()) {
      const hls = new Hls();
      hls.loadSource(src);
      hls.attachMedia(video);
      if (isGif) {
        hls.on(Hls.Events.MANIFEST_PARSED, () => {
          video.play().catch(() => {});
        });
      }
      return () => hls.destroy();
    }

    if (video.canPlayType("application/vnd.apple.mpegurl")) {
      video.src = src;
      // HLSはsrcをJS側で遅延セットするため、マウント時点のautoPlay属性だけでは
      // 再生が始まらない（srcが無い状態でブラウザに解釈されるため）。GIF表示は
      // 明示的にplay()を呼んで開始する。
      if (isGif) {
        video.play().catch(() => {});
      }
    }
  }, [src, isHls, isGif]);

  const hasFallback = !isHls && !!fallbackSrc && fallbackSrc !== src;

  return (
    <video
      ref={videoRef}
      src={isHls || hasFallback ? undefined : src}
      poster={poster}
      controls={!isGif}
      autoPlay={isGif}
      muted={isGif}
      loop={isGif}
      playsInline={isGif}
      className={className}
      onClick={onClick}
    >
      {hasFallback && (
        <>
          <source src={src} type={mimeType} />
          <source src={fallbackSrc} type={mimeType} />
        </>
      )}
    </video>
  );
}
