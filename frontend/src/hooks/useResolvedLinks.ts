import { useEffect, useState } from "react";
import { ResolvedLinkInfo } from "../api/types";
import { useStreamingContext } from "../contexts/StreamingContext";

/**
 * bio/profile_fields内リンクの解決結果（#リンク解決）。key=URL文字列。初期値はAPIレスポンス
 * 同梱分（`link_resolutions`）、非同期解決の完了は`linkResolved`のWebSocket通知で追記される。
 * 通知はログイン中クライアント全員へのブロードキャストのため、URLでのフィルタはせず単純に
 * マージする（表示中のHTMLに含まれないURLは`RichHtml`側で素通りする）。
 */
export function useResolvedLinks(initial: Record<string, ResolvedLinkInfo> | undefined) {
  const [resolvedLinks, setResolvedLinks] = useState<Record<string, ResolvedLinkInfo>>(
    initial ?? {},
  );
  const { registerLinkResolved } = useStreamingContext();

  useEffect(() => {
    return registerLinkResolved((info) => {
      setResolvedLinks((prev) => ({
        ...prev,
        [info.url]: {
          kind: info.kind,
          username: info.username,
          domain: info.domain,
          actor_type: info.actorType,
          actor_id: info.actorId,
          avatar_url: info.avatarUrl,
          post_id: info.postId,
        },
      }));
    });
  }, [registerLinkResolved]);

  return [resolvedLinks, setResolvedLinks] as const;
}
