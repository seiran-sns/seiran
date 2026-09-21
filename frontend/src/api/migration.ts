// 既存DID転入フロー（Blueskyアカウントの移行）専用のAPIクライアント。
//
// `submitting_plc`成功までアカウントが存在しないため、通常の`request()`（JWT付与）とは
// 別経路: `X-Migration-Token`ヘッダで認可する。トークンはこのモジュールを呼ぶ側
// （ページコンポーネント）がlocalStorage等で保持する。

import { throwIfError, parseJsonBody } from "./core";
import type { AuthResponse } from "./types";

const BASE = "/api";

export interface MigrationStartParams {
  source_handle: string;
  source_password: string;
  new_username: string;
  new_password: string;
  auth_factor_token?: string;
}

export interface MigrationStartResponse {
  /** snowflake IDはJSの53bit整数精度を超えるため文字列で返る。数値化せず文字列のまま扱うこと。 */
  request_id: string;
  request_token: string;
  status: string;
}

export interface MigrationStatusResponse {
  request_id: string;
  status: string;
  last_error?: string | null;
  /** `"plc_token"` | null。フロントが表示すべき入力欄の種類。 */
  needs_input?: string | null;
  retryable: boolean;
  can_abandon: boolean;
  /** `status === "importing_data"`のときのみ非null（取り込み済み件数/全体件数）。 */
  import_done?: number | null;
  import_total?: number | null;
}

interface SimpleStatus {
  status: string;
}

async function migrationRequest<T>(
  method: string,
  path: string,
  token?: string,
  body?: unknown,
): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      ...(token ? { "X-Migration-Token": token } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  // notifyUnauthorized=false: このトークンはJWTと無関係のため、AuthContextの
  // グローバルログアウト誘導（401時）を発火させない。
  await throwIfError(res, false);
  return parseJsonBody<T>(res);
}

export const migration = {
  start(params: MigrationStartParams) {
    return migrationRequest<MigrationStartResponse>("POST", "/migration/start", undefined, params);
  },
  submitPlcToken(id: string, requestToken: string, plcToken: string) {
    return migrationRequest<AuthResponse>("POST", `/migration/${id}/submit-plc-token`, requestToken, {
      token: plcToken,
    });
  },
  status(id: string, requestToken: string) {
    return migrationRequest<MigrationStatusResponse>("GET", `/migration/${id}/status`, requestToken);
  },
  retry(id: string, requestToken: string) {
    return migrationRequest<SimpleStatus>("POST", `/migration/${id}/retry`, requestToken);
  },
  abandon(id: string, requestToken: string) {
    return migrationRequest<SimpleStatus>("POST", `/migration/${id}/abandon`, requestToken);
  },
};
