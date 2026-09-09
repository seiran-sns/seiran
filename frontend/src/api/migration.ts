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
  email?: string;
}

export interface MigrationStartResponse {
  request_id: number;
  request_token: string;
  status: string;
}

export interface MigrationStatusResponse {
  request_id: number;
  status: string;
  last_error?: string | null;
  /** `"seiran_email_token"` | `"plc_token"` | null。フロントが表示すべき入力欄の種類。 */
  needs_input?: string | null;
  retryable: boolean;
  can_abandon: boolean;
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
  confirmSeiranEmail(id: number, requestToken: string, registrationToken: string) {
    return migrationRequest<SimpleStatus>(
      "POST",
      `/migration/${id}/confirm-seiran-email`,
      requestToken,
      { registration_token: registrationToken },
    );
  },
  submitPlcToken(id: number, requestToken: string, plcToken: string) {
    return migrationRequest<AuthResponse>("POST", `/migration/${id}/submit-plc-token`, requestToken, {
      token: plcToken,
    });
  },
  status(id: number, requestToken: string) {
    return migrationRequest<MigrationStatusResponse>("GET", `/migration/${id}/status`, requestToken);
  },
  retry(id: number, requestToken: string) {
    return migrationRequest<SimpleStatus>("POST", `/migration/${id}/retry`, requestToken);
  },
  abandon(id: number, requestToken: string) {
    return migrationRequest<SimpleStatus>("POST", `/migration/${id}/abandon`, requestToken);
  },
};
