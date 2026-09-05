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
    // して502になる不具合があった。
    const proxy = externalProxyBase || "/proxy";
    return `${proxy}?url=${encodeURIComponent(target.href)}`;
  } catch {
    return raw;
  }
}
