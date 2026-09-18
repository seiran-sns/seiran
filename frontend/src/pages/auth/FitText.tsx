import { CSSProperties } from "react";
import { useFitTextSize } from "../../hooks/useFitTextSize";

interface FitTextProps {
  html: string;
  maxPx: number;
  className?: string;
}

/**
 * `html`（管理者専用入力のため未サニタイズのまま描画）を、改行させずコンテナ幅へ収まる
 * 最大のfont-size（`maxPx`が上限）で表示する（issue #243のサイトタイトル自動フィット）。
 */
export default function FitText({ html, maxPx, className }: FitTextProps) {
  const { containerRef, measureRef, fontSize } = useFitTextSize(maxPx, [html]);

  const hiddenStyle: CSSProperties = {
    position: "absolute",
    visibility: "hidden",
    whiteSpace: "nowrap",
    fontSize: maxPx,
    pointerEvents: "none",
  };
  const visibleStyle: CSSProperties = {
    display: "inline-block",
    whiteSpace: "nowrap",
    fontSize,
    lineHeight: 1.1,
  };

  return (
    <div ref={containerRef} className={className} style={{ overflow: "hidden", width: "100%" }}>
      <span ref={measureRef} style={hiddenStyle} dangerouslySetInnerHTML={{ __html: html }} />
      <span style={visibleStyle} dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
}
