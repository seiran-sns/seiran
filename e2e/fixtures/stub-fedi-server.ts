// リモートのActivityPubアクター（Mastodon等）を模擬するスタブ実装。E2Eで
// 「seiranユーザーをフォローする → 投稿/返信/リポストの配送を受け取る」までを検証する。
//
// seiran側の署名検証（crates/seiran-federation-inbox/src/handlers/inbox.rs、
// crates/seiran-common/src/ap/client.rs の verify_signature/build_signing_string）は
// 正規のHTTP Signatures（RSA-SHA256、Digestヘッダー必須）を要求するため、フォロー送信時も
// 同じ規約で実署名する。受信（inbox）側はseiranが送ってくる署名の検証はせず、ただ記録する。

import { createServer, type Server } from "node:http";
import { createHash, createSign, generateKeyPairSync } from "node:crypto";

export interface StubFediServer {
  url: string;
  actorUri: string;
  inboxUrl: string;
  /** 受信した生アクティビティ（Accept・Create・Announce等）を新着順に返す。 */
  receivedActivities(): Record<string, unknown>[];
  /** このスタブアクターからseiranへ署名付きFollowを送り、対象ローカルユーザーをフォローする。 */
  sendFollow(seiranBaseUrl: string, targetUsername: string): Promise<void>;
  close(): Promise<void>;
}

// crates/seiran-common/src/ap/client.rs の build_signing_string と同じ規約
// （"(request-target): {method} {path}" ＋ 各ヘッダーを "name: value" で改行結合）。
function buildSigningString(method: string, path: string, headersOrder: string[], values: Record<string, string>): string {
  return headersOrder
    .map((h) => (h === "(request-target)" ? `(request-target): ${method.toLowerCase()} ${path}` : `${h}: ${values[h]}`))
    .join("\n");
}

async function signedPost(targetUrl: string, activity: unknown, actorUri: string, privateKeyPem: string): Promise<void> {
  const body = JSON.stringify(activity);
  const url = new URL(targetUrl);
  const digest = `SHA-256=${createHash("sha256").update(body).digest("base64")}`;
  const date = new Date().toUTCString();
  const headersOrder = ["(request-target)", "host", "date", "digest"];
  const values: Record<string, string> = { host: url.host, date, digest };
  const signingString = buildSigningString("POST", url.pathname, headersOrder, values);

  const signer = createSign("RSA-SHA256");
  signer.update(signingString);
  signer.end();
  const signatureB64 = signer.sign(privateKeyPem).toString("base64");
  const signatureHeader = `keyId="${actorUri}#main-key",algorithm="rsa-sha256",headers="${headersOrder.join(" ")}",signature="${signatureB64}"`;

  const res = await fetch(targetUrl, {
    method: "POST",
    headers: {
      "content-type": "application/activity+json",
      host: url.host,
      date,
      digest,
      signature: signatureHeader,
    },
    body,
  });
  if (!res.ok) {
    throw new Error(`signed POST to ${targetUrl} failed: ${res.status} ${await res.text()}`);
  }
}

export function startStubFediServer(port = 0): Promise<StubFediServer> {
  const { publicKey, privateKey } = generateKeyPairSync("rsa", {
    modulusLength: 2048,
    publicKeyEncoding: { type: "spki", format: "pem" },
    privateKeyEncoding: { type: "pkcs8", format: "pem" },
  });

  const received: Record<string, unknown>[] = [];
  let stub: StubFediServer;

  const server: Server = createServer((req, res) => {
    const path = (req.url ?? "").split("?")[0];

    if (req.method === "GET" && path === "/actor") {
      const doc = {
        "@context": ["https://www.w3.org/ns/activitystreams", "https://w3id.org/security/v1"],
        id: stub.actorUri,
        type: "Person",
        preferredUsername: "e2efedibot",
        name: "E2E Fedi Bot",
        inbox: stub.inboxUrl,
        publicKey: { id: `${stub.actorUri}#main-key`, owner: stub.actorUri, publicKeyPem: publicKey },
      };
      res.writeHead(200, { "content-type": "application/activity+json" }).end(JSON.stringify(doc));
      return;
    }

    if (req.method === "POST" && path === "/inbox") {
      const chunks: Buffer[] = [];
      req.on("data", (c) => chunks.push(c));
      req.on("end", () => {
        const raw = Buffer.concat(chunks).toString("utf8");
        try {
          received.unshift(JSON.parse(raw));
        } catch (err) {
          received.unshift({ _parseError: String(err), raw });
        }
        res.writeHead(202).end();
      });
      return;
    }

    res.writeHead(404).end();
  });

  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        throw new Error("stub Fedi server のアドレス取得に失敗しました");
      }
      const base = `http://127.0.0.1:${address.port}`;
      stub = {
        url: base,
        actorUri: `${base}/actor`,
        inboxUrl: `${base}/inbox`,
        receivedActivities: () => received,
        async sendFollow(seiranBaseUrl, targetUsername) {
          const targetActorUri = `${seiranBaseUrl}/users/${targetUsername}`;
          const activity = {
            "@context": "https://www.w3.org/ns/activitystreams",
            type: "Follow",
            id: `${base}/follows/${Date.now()}`,
            actor: stub.actorUri,
            object: targetActorUri,
          };
          await signedPost(`${seiranBaseUrl}/inbox`, activity, stub.actorUri, privateKey);
        },
        close: () => new Promise((res, rej) => server.close((err) => (err ? rej(err) : res()))),
      };
      resolve(stub);
    });
  });
}
