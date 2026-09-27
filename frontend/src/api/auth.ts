import { request } from "./core";
import type { AuthResponse, LoginResult, SetupStatus, User, VerifyEmailResponse, VerifyTokenResponse } from "./types";
import { authenticationOptions, credentialJson } from "./webauthn";
import type { AuthenticationOptionsJson, WebAuthnEnvelope } from "./webauthn";

export const setup = {
  status(signal?: AbortSignal) {
    return request<SetupStatus>("GET", "/setup/status", undefined, signal);
  },
  initialize(
    username: string,
    email: string,
    password: string,
    domainCandidate: string | null,
  ) {
    return request<AuthResponse>("POST", "/setup", {
      username,
      email,
      password,
      domain_candidate: domainCandidate,
    });
  },
};

export const auth = {
  requestEmailVerification(email: string, turnstileToken?: string) {
    return request<VerifyEmailResponse>("POST", "/auth/verify-email", {
      email,
      turnstile_token: turnstileToken,
    });
  },
  verifyEmailToken(token: string, signal?: AbortSignal) {
    return request<VerifyTokenResponse>(
      "GET",
      `/auth/verify-token?token=${encodeURIComponent(token)}`,
      undefined,
      signal,
    );
  },
  register(
    username: string,
    password: string,
    registrationToken: string,
    turnstileToken?: string,
    birthday?: string,
  ) {
    return request<AuthResponse>("POST", "/auth/register", {
      username,
      password,
      registration_token: registrationToken,
      turnstile_token: turnstileToken,
      birthday: birthday || undefined,
    });
  },
  registerDirect(
    email: string,
    username: string,
    password: string,
    turnstileToken?: string,
    birthday?: string,
  ) {
    return request<AuthResponse>("POST", "/auth/register", {
      username,
      password,
      email,
      turnstile_token: turnstileToken,
      birthday: birthday || undefined,
    });
  },
  login(identifier: string, password: string, turnstileToken?: string) {
    return request<LoginResult>("POST", "/auth/login", {
      identifier,
      password,
      turnstile_token: turnstileToken,
    });
  },
  async loginWithPasskey() {
    const start = await request<WebAuthnEnvelope<AuthenticationOptionsJson>>(
      "POST",
      "/auth/passkeys/start",
    );
    const credential = (await navigator.credentials.get({
      publicKey: authenticationOptions(start.public_key.publicKey),
    })) as PublicKeyCredential | null;
    if (!credential) throw new Error("Passkey authentication was cancelled");
    return request<AuthResponse>("POST", "/auth/passkeys/finish", {
      token: start.token,
      credential: credentialJson(credential),
    });
  },
  me() {
    // /auth/me は AuthContext がリトライ結果を見て認証失効を判断する。
    // ここでグローバル401ハンドラを発火すると、一時的な401でも確認前に
    // トークンを破棄してしまうため通知を抑止する（#108）。
    return request<User>("GET", "/auth/me", undefined, undefined, false);
  },
  requestPasswordReset(email: string) {
    return request<{ message: string }>(
      "POST",
      "/auth/request-password-reset",
      { email },
    );
  },
  verifyResetToken(token: string, signal?: AbortSignal) {
    return request<{ valid: boolean }>(
      "GET",
      `/auth/verify-reset-token?token=${encodeURIComponent(token)}`,
      undefined,
      signal,
    );
  },
  resetPassword(token: string, newPassword: string) {
    return request<{ message: string }>("POST", "/auth/reset-password", {
      token,
      new_password: newPassword,
    });
  },
  totp: {
    /** ログイン2段階目（#65）。`code`はTOTPコード（6桁数字）またはリカバリーコード（`nnnn-nnnn`）。 */
    verify(pendingToken: string, code: string) {
      return request<AuthResponse>("POST", "/auth/totp/verify", {
        pending_token: pendingToken,
        code,
      });
    },
    /** 認証アプリ・リカバリーコードを両方失った場合、登録メールアドレス宛に解除リンクを送る（#65）。 */
    requestDisableEmail(pendingToken: string) {
      return request<void>("POST", "/auth/totp/request-disable-email", {
        pending_token: pendingToken,
      });
    },
    /** メールのリンク（`/totp-disable?token=...`）を踏んだ際にトークンを確定する（#65）。 */
    confirmDisable(token: string) {
      return request<void>("POST", "/auth/totp/confirm-disable", { token });
    },
  },
};

/** Mastodon 互換 OAuth の認可要求（`GET /oauth/authorize` のクエリをそのまま引き継いだもの）。 */
export interface OAuthAuthorizeParams {
  client_id: string;
  redirect_uri: string;
  response_type?: string;
  scope?: string;
  state?: string;
  code_challenge?: string;
  code_challenge_method?: string;
}

export const oauth = {
  /** 承認画面に出すアプリ名（URL のクエリではなく登録内容から引く）。 */
  appInfo(clientId: string) {
    return request<{ name: string; website: string | null }>(
      "GET",
      `/oauth/apps/${encodeURIComponent(clientId)}`,
    );
  },
  /**
   * OAuth 承認画面（`/oauth-connect`）で「承認する」を押した時に呼ぶ。`redirectUrl` が
   * `null` なのは OOB（コードを画面に表示してユーザーがアプリへ手入力する方式）。
   */
  async authorize(params: OAuthAuthorizeParams) {
    const res = await request<{ redirect_url: string | null; code: string }>(
      "POST",
      "/oauth/authorize",
      params,
    );
    return { redirectUrl: res.redirect_url, code: res.code };
  },
};

export const miauth = {
  /**
   * MiAuth 認可確認画面（`/connect/:sessionId`）で「承認する」を押した時に呼ぶ。
   * `name` はクライアントアプリ名（#60: 発行済みトークン一覧に表示する）。
   */
  authorize(sessionId: string, name?: string) {
    return request<{ ok: boolean }>(
      "POST",
      `/miauth/${encodeURIComponent(sessionId)}/authorize`,
      { name },
    );
  },
};

/** Misskey 旧来の app 認証フロー（app/create 等）の承認確認画面が使う参照情報。 */
export interface MisskeyAuthSessionInfo {
  name: string;
  description: string | null;
  permission: string[];
  callbackUrl: string | null;
}

export const misskeyAppAuth = {
  /** 承認確認画面に出すアプリ名（URL のクエリではなく登録内容から引く）。 */
  sessionInfo(token: string) {
    return request<MisskeyAuthSessionInfo>(
      "GET",
      `/auth-sessions/${encodeURIComponent(token)}`,
    );
  },
  /** 承認確認画面（`/misskey-connect/:token`）で「承認する」を押した時に呼ぶ。 */
  authorize(token: string) {
    return request<{ ok: boolean }>(
      "POST",
      `/auth-sessions/${encodeURIComponent(token)}/authorize`,
    );
  },
};
