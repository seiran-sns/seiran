// AT Protocol OAuth の最小のクライアント（E2E 用）。DPoP 鍵の生成・証明の署名・nonce の
// 取り直し・PKCE を行う。開発用の `http://localhost` クライアント（メタデータの取得が要らない）を使う。

import { createHash, randomBytes, webcrypto } from "node:crypto";
import type { APIRequestContext, APIResponse } from "@playwright/test";
import { BACKEND_URL, LOCAL_DOMAIN } from "../ports.ts";

const subtle = webcrypto.subtle;

/** DPoP 証明の `htu`。サーバーは外から見た URL（`https://{LOCAL_DOMAIN}`）と照合する。 */
export const ISSUER = `https://${LOCAL_DOMAIN}`;
export const REDIRECT_URI = "http://127.0.0.1/callback";

function b64url(data: Uint8Array | string): string {
  return Buffer.from(data).toString("base64url");
}

export function loopbackClientId(scope: string): string {
  return `http://localhost?redirect_uri=${encodeURIComponent(REDIRECT_URI)}&scope=${encodeURIComponent(scope)}`;
}

export class DpopKey {
  private constructor(
    private readonly privateKey: webcrypto.CryptoKey,
    readonly publicJwk: { kty: string; crv: string; x: string; y: string },
  ) {}

  static async generate(): Promise<DpopKey> {
    const pair = await subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
    const jwk = await subtle.exportKey("jwk", pair.publicKey);
    return new DpopKey(pair.privateKey, { kty: "EC", crv: "P-256", x: jwk.x!, y: jwk.y! });
  }

  async proof(htm: string, htu: string, opts: { nonce?: string; accessToken?: string; jti?: string } = {}) {
    const header = { typ: "dpop+jwt", alg: "ES256", jwk: this.publicJwk };
    const payload: Record<string, unknown> = {
      jti: opts.jti ?? b64url(randomBytes(16)),
      htm,
      htu,
      iat: Math.floor(Date.now() / 1000),
    };
    if (opts.nonce) payload.nonce = opts.nonce;
    if (opts.accessToken) payload.ath = b64url(createHash("sha256").update(opts.accessToken).digest());
    const input = `${b64url(JSON.stringify(header))}.${b64url(JSON.stringify(payload))}`;
    // WebCrypto の ECDSA 署名は r||s の 64 バイトで、JWS の ES256 と同じ形。
    const sig = await subtle.sign({ name: "ECDSA", hash: "SHA-256" }, this.privateKey, Buffer.from(input));
    return `${input}.${b64url(new Uint8Array(sig))}`;
  }
}

export function pkce() {
  const verifier = b64url(randomBytes(32));
  const challenge = b64url(createHash("sha256").update(verifier).digest());
  return { verifier, challenge };
}

/** トークン系エンドポイントへ DPoP 付きで POST する。nonce を求められたら1回だけ取り直す。 */
export async function postWithDpop(
  request: APIRequestContext,
  key: DpopKey,
  path: string,
  form: Record<string, string>,
  state: { nonce?: string },
): Promise<APIResponse> {
  for (let attempt = 0; ; attempt++) {
    const res = await request.post(`${BACKEND_URL}${path}`, {
      form,
      headers: { DPoP: await key.proof("POST", `${ISSUER}${path}`, { nonce: state.nonce }) },
    });
    const nonce = res.headers()["dpop-nonce"];
    if (nonce) state.nonce = nonce;
    if (attempt === 0 && res.status() === 400 && (await res.json()).error === "use_dpop_nonce") continue;
    return res;
  }
}

/** XRPC を OAuth のアクセストークンで呼ぶ。nonce を求められたら1回だけ取り直す。 */
export async function xrpcWithDpop(
  request: APIRequestContext,
  key: DpopKey,
  method: "GET" | "POST",
  nsid: string,
  accessToken: string,
  state: { nonce?: string },
  data?: unknown,
): Promise<APIResponse> {
  const path = `/xrpc/${nsid}`;
  for (let attempt = 0; ; attempt++) {
    const headers = {
      Authorization: `DPoP ${accessToken}`,
      DPoP: await key.proof(method, `${ISSUER}${path}`, { nonce: state.nonce, accessToken }),
    };
    const res =
      method === "GET"
        ? await request.get(`${BACKEND_URL}${path}`, { headers })
        : await request.post(`${BACKEND_URL}${path}`, { headers, data });
    const nonce = res.headers()["dpop-nonce"];
    if (nonce) state.nonce = nonce;
    if (attempt === 0 && res.status() === 401 && (res.headers()["www-authenticate"] ?? "").includes("use_dpop_nonce")) {
      continue;
    }
    return res;
  }
}
