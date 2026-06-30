const BASE = "/api";

function getToken(): string | null {
  return localStorage.getItem("seiran_token");
}

function authHeaders(): Record<string, string> {
  const token = getToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown
): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      "Content-Type": "application/json",
      ...authHeaders(),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });

  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
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

export const api = {
  auth: {
    register(username: string, email: string, password: string) {
      return request<AuthResponse>("POST", "/auth/register", {
        username,
        email,
        password,
      });
    },
    login(email: string, password: string) {
      return request<AuthResponse>("POST", "/auth/login", { email, password });
    },
    me() {
      return request<User>("GET", "/auth/me");
    },
  },

  notes: {
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
