const BASE = "/api";

function getToken(): string | null {
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
    public readonly detail?: Record<string, unknown>
  ) {
    super(code);
    this.name = "ApiError";
  }
}

const ERROR_MESSAGES: Record<string, string> = {
  EMAIL_ALREADY_REGISTERED: "このメールアドレスはすでに使用されています",
  EMAIL_INVALID: "有効なメールアドレスを入力してください",
  INVALID_TOKEN: "リンクが無効か期限切れです。メールの確認をやり直してください",
  USERNAME_TAKEN: "このユーザー名はすでに使用されています",
  INVALID_INPUT: "入力内容を確認してください",
  ALREADY_INITIALIZED: "セットアップはすでに完了しています",
  INVALID_CREDENTIALS: "メールアドレスまたはパスワードが正しくありません",
  REGISTRATION_TOKEN_INVALID: "メール確認をやり直してください",
  UNAUTHORIZED: "ログインが必要です",
  NOT_FOUND: "見つかりません",
  INTERNAL_ERROR: "サーバーエラーが発生しました。しばらく待ってから再試行してください",
  RESET_TOKEN_INVALID: "リンクが無効または期限切れです。パスワードリセットをやり直してください",
  PASSWORD_TOO_SHORT: "パスワードは8文字以上で入力してください",
};

export function getErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    return ERROR_MESSAGES[error.code] ?? `エラーが発生しました (${error.code})`;
  }
  if (error instanceof Error) return error.message;
  return "不明なエラーが発生しました";
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  signal?: AbortSignal
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

  if (!res.ok) {
    const contentType = res.headers.get("content-type") ?? "";
    if (contentType.includes("application/json")) {
      try {
        const err = (await res.json()) as { code?: string; detail?: Record<string, unknown> };
        if (err.code) {
          throw new ApiError(err.code, res.status, err.detail);
        }
      } catch (e) {
        if (e instanceof ApiError) throw e;
      }
    }
    throw new ApiError("UNKNOWN_ERROR", res.status);
  }

  return res.json() as Promise<T>;
}

// =====================================================================
// 型定義
// =====================================================================

export interface User {
  id: number;
  username: string;
  email: string;
}

export interface AuthResponse {
  token: string;
  user: User;
}

export interface Note {
  id: string;
  text: string;
  created_at: string;
  user: {
    id: number;
    username: string;
    domain?: string;
    display_name?: string;
  };
}

export interface ProfileNote {
  id: string;
  text: string;
  created_at: string;
}

export interface UserProfile {
  username: string;
  domain: string;
  display_name?: string;
  actor_type: string;
  ap_uri?: string;
  follow_status: "not_following" | "pending" | "accepted";
  recent_posts: ProfileNote[];
}

export interface FollowResponse {
  status: string;
  target_uri: string;
}

// =====================================================================
// Auth API
// =====================================================================

export interface VerifyEmailResponse {
  message: string;
}

export interface VerifyTokenResponse {
  registration_token: string;
}

export interface SetupStatus {
  initialized: boolean;
}

export const api = {
  setup: {
    status(signal?: AbortSignal) {
      return request<SetupStatus>("GET", "/setup/status", undefined, signal);
    },
    initialize(username: string, email: string, password: string) {
      return request<AuthResponse>("POST", "/setup", { username, email, password });
    },
  },

  auth: {
    requestEmailVerification(email: string) {
      return request<VerifyEmailResponse>("POST", "/auth/verify-email", { email });
    },
    verifyEmailToken(token: string, signal?: AbortSignal) {
      return request<VerifyTokenResponse>("GET", `/auth/verify-token?token=${encodeURIComponent(token)}`, undefined, signal);
    },
    register(username: string, password: string, registrationToken: string) {
      return request<AuthResponse>("POST", "/auth/register", {
        username,
        password,
        registration_token: registrationToken,
      });
    },
    login(email: string, password: string) {
      return request<AuthResponse>("POST", "/auth/login", { email, password });
    },
    me() {
      return request<User>("GET", "/auth/me");
    },
    requestPasswordReset(email: string) {
      return request<{ message: string }>("POST", "/auth/request-password-reset", { email });
    },
    verifyResetToken(token: string, signal?: AbortSignal) {
      return request<{ valid: boolean }>("GET", `/auth/verify-reset-token?token=${encodeURIComponent(token)}`, undefined, signal);
    },
    resetPassword(token: string, newPassword: string) {
      return request<{ message: string }>("POST", "/auth/reset-password", { token, new_password: newPassword });
    },
  },

  notes: {
    get(id: string) {
      return request<Note>("GET", `/notes/${encodeURIComponent(id)}`);
    },
    create(text: string) {
      return request<Note>("POST", "/notes/create", { text });
    },
    localTimeline(params?: { limit?: number; until_id?: string; since_id?: string }) {
      const q = new URLSearchParams();
      if (params?.limit) q.set("limit", String(params.limit));
      if (params?.until_id) q.set("until_id", params.until_id);
      if (params?.since_id) q.set("since_id", params.since_id);
      const qs = q.toString();
      return request<Note[]>("GET", `/notes/local-timeline${qs ? `?${qs}` : ""}`);
    },
    homeTimeline(params?: { limit?: number; until_id?: string; since_id?: string }) {
      const q = new URLSearchParams();
      if (params?.limit) q.set("limit", String(params.limit));
      if (params?.until_id) q.set("until_id", params.until_id);
      if (params?.since_id) q.set("since_id", params.since_id);
      const qs = q.toString();
      return request<Note[]>("GET", `/notes/home-timeline${qs ? `?${qs}` : ""}`);
    },
  },

  users: {
    profile(q: string) {
      return request<UserProfile>("GET", `/users/profile?q=${encodeURIComponent(q)}`);
    },
  },

  follows: {
    create(target: string) {
      return request<FollowResponse>("POST", "/follows/create", { target });
    },
  },
};

export { getToken };
