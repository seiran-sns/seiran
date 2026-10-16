let externalProxyBase = "";
let internalMediaOrigins = new Set<string>();

export function configureMediaProxy(base: string) {
  externalProxyBase = base.trim().replace(/\/+$/, "");
}

/**
 * 自インスタンスが管理するストレージのオリジン（R2等のカスタムドメイン）を登録する。
 * `window.location.origin`とは別サブドメインで運用されることが多く、
 * SSRF対策・容量上限付きの`/proxy`を通す必要がない（自分のインフラのため）。
 */
export function configureInternalMediaOrigins(origins: string[]) {
  internalMediaOrigins = new Set(
    origins
      .map((o) => {
        try {
          return new URL(o).origin;
        } catch {
          return null;
        }
      })
      .filter((o): o is string => o !== null),
  );
}

/** 同一オリジン・自インスタンスのストレージオリジン・非HTTP URLはそのままにし、リモートメディアだけをプロキシへ通す。 */
export function mediaUrl(raw?: string | null): string | undefined {
  if (!raw) return undefined;
  try {
    const target = new URL(raw, window.location.origin);
    if (
      !["http:", "https:"].includes(target.protocol) ||
      target.origin === window.location.origin ||
      internalMediaOrigins.has(target.origin)
    )
      return raw;
    // `externalProxyBase`はMisskeyの`instance.mediaProxy`互換で、`/proxy`まで含む
    // 完全なエンドポイントURL（例: `https://example.com/proxy`）としてそのまま使う。
    // ここでさらに`/proxy`を付け足すと、`/api/meta`の`mediaProxyUrl`未設定時デフォルト
    // （`https://{local_domain}/proxy`）と組み合わさって`/proxy/proxy?url=...`という
    // 二重パスになり、nginx側にそのパス用のルーティングがなくフロントへ誤フォールバック
    // して502になる。
    const proxy = externalProxyBase || "/proxy";
    return `${proxy}?url=${encodeURIComponent(target.href)}`;
  } catch {
    return raw;
  }
}

/** 外部メディアを試す優先順位。`direct`は元URL優先、`proxy`はメディアプロキシ経由優先。 */
export type MediaPrefer = "direct" | "proxy";

/**
 * 外部メディアの候補URLを優先順に返す。直リンクもプロキシ経由も失敗しうる
 * （直: CORS・ホットリンク拒否・混在コンテンツ、プロキシ: 想定外形式の拒否・サイズ上限）ため、
 * 両方を順に試せるよう並べる。同一オリジン等でプロキシを通さないURLは1本だけ返す。
 * `prefer`は、配信元がタイムラインからの一斉アクセスを想定しているか（SNSサーバーの
 * アイコン・絵文字・添付は想定済みなので`direct`、OGPサムネイルの出元は未想定なので`proxy`）で選ぶ。
 */
export function mediaSources(raw?: string | null, prefer: MediaPrefer = "direct"): string[] {
  if (!raw) return [];
  const proxied = mediaUrl(raw);
  if (!proxied || proxied === raw) return [raw];
  return prefer === "proxy" ? [proxied, raw] : [raw, proxied];
}
