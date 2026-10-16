import { useState, type ImgHTMLAttributes } from "react";
import { mediaSources, type MediaPrefer } from "../../utils/mediaProxy";

interface RemoteImgProps extends Omit<ImgHTMLAttributes<HTMLImageElement>, "src"> {
  src?: string | null;
  /** 直リンクとメディアプロキシ経由のどちらを先に試すか（`mediaSources`参照）。 */
  prefer?: MediaPrefer;
}

/**
 * 外部サーバー上の画像。直リンクとプロキシ経由URLを優先順に試し、読み込みに失敗したら
 * 次の候補へ切り替える。全候補が失敗した時だけ呼び出し側の`onError`を呼ぶ。
 */
export default function RemoteImg({ src, prefer = "direct", onError, ...rest }: RemoteImgProps) {
  const [state, setState] = useState({ src, index: 0 });
  // 表示対象が変わったら候補の試行をやり直す（再利用された要素に前回の失敗を引き継がない）。
  const index = state.src === src ? state.index : 0;
  const sources = mediaSources(src, prefer);

  return (
    <img
      {...rest}
      src={sources[index]}
      onError={(e) => {
        if (index + 1 < sources.length) {
          setState({ src, index: index + 1 });
        } else {
          onError?.(e);
        }
      }}
    />
  );
}
