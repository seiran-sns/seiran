import i18n from "../i18n";

const BASE = "/api";

export function getToken(): string | null {
  return localStorage.getItem("seiran_token");
}

function authHeaders(): Record<string, string> {
  const token = getToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

// =====================================================================
// 構造化エラー
// API は {"code": "...", "detail": {...}} の JSON を返す。
// フロントエンドが code を見てユーザー向けメッセージに変換する責務を持つ。
// =====================================================================

export class ApiError extends Error {
  constructor(
    public readonly code: string,
    public readonly status: number,
    public readonly detail?: Record<string, unknown>,
  ) {
    super(code);
    this.name = "ApiError";
  }
}

/**
 * バックエンドのエラーコード（`crates/seiran-api/src/error.rs`）を
 * `errors.*`（`frontend/src/i18n/locales/{lng}/errors.json`）へ機械的に対応させる。
 * 未知のコードは 5xx なら SERVER_UNAVAILABLE、それ以外は UNKNOWN_WITH_CODE にフォールバックする。
 */
export function getErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    const key = `errors:${error.code}`;
    if (i18n.exists(key)) return i18n.t(key);
    if (error.status >= 500) return i18n.t("errors:SERVER_UNAVAILABLE");
    return i18n.t("errors:UNKNOWN_WITH_CODE", { code: error.code });
  }
  // fetch 自体が失敗した場合（オフライン・DNS 失敗等）は TypeError になる。
  if (error instanceof TypeError) return i18n.t("errors:NETWORK_ERROR");
  if (error instanceof Error) return error.message;
  return i18n.t("errors:UNKNOWN");
}

type UnauthorizedHandler = () => void;
let unauthorizedHandler: UnauthorizedHandler | null = null;

/**
 * トークン失効時（401）のグローバル処理（ログアウト＋ログイン画面誘導）を登録する。
 * `AuthProvider` がマウント時に登録する。ログイン試行自体の 401（認証情報間違い）では
 * トークンが存在しないため発火しない。
 */
export function setUnauthorizedHandler(handler: UnauthorizedHandler | null) {
  unauthorizedHandler = handler;
}

function notifyIfUnauthorized(status: number) {
  if (status === 401 && getToken()) {
    unauthorizedHandler?.();
  }
}

/** レスポンスが失敗（`!res.ok`）であれば `ApiError` を投げる（`request`/`uploadFormData` で共通）。 */
export async function throwIfError(
  res: Response,
  notifyUnauthorized = true,
): Promise<void> {
  if (res.ok) return;
  if (notifyUnauthorized) notifyIfUnauthorized(res.status);
  const contentType = res.headers.get("content-type") ?? "";
  if (contentType.includes("application/json")) {
    try {
      const err = (await res.json()) as {
        code?: string;
        detail?: Record<string, unknown>;
      };
      if (err.code) {
        throw new ApiError(err.code, res.status, err.detail);
      }
    } catch (e) {
      if (e instanceof ApiError) throw e;
    }
  }
  throw new ApiError("UNKNOWN_ERROR", res.status);
}

/**
 * 成功レスポンスのボディを JSON としてパースする（`request`/`uploadFormData` で共通）。
 * 204 No Content 等、ボディが無い成功レスポンスは `res.json()` が
 * "Unexpected end of JSON input" で例外を投げるため、パース前に弾く
 * （例: admin のロール変更/凍結・解除 API。処理自体は成功しているのに
 * 呼び出し側にエラーとして伝播していた不具合）。
 */
export async function parseJsonBody<T>(res: Response): Promise<T> {
  if (res.status === 204) {
    return undefined as T;
  }
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal,
  notifyUnauthorized = true,
): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      ...authHeaders(),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
    signal,
  });
  await throwIfError(res, notifyUnauthorized);
  return parseJsonBody<T>(res);
}

/** FormData 送信（`request()` は JSON body 前提のため通せない）用の共通エラーハンドリング。 */
export async function uploadFormData<T>(path: string, formData: FormData): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method: "POST",
    headers: { ...authHeaders() },
    body: formData,
  });
  await throwIfError(res);
  return parseJsonBody<T>(res);
}

/** limit/until_id/since_id カーソルパラメータを組み立てる（7箇所の重複を共通化）。 */
export function cursorParams(params?: {
  limit?: number;
  until_id?: string;
  since_id?: string;
}): URLSearchParams {
  const q = new URLSearchParams();
  if (params?.limit) q.set("limit", String(params.limit));
  if (params?.until_id) q.set("until_id", params.until_id);
  if (params?.since_id) q.set("since_id", params.since_id);
  return q;
}
