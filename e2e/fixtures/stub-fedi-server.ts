// リモートのActivityPubアクター（Mastodon等）を模擬するスタブ実装。E2Eで
// 「seiranユーザーをフォローする → 投稿/返信/リポストの配送を受け取る」までを検証する。
//
// seiran側の署名検証（crates/seiran-federation-inbox/src/handlers/inbox.rs、
// crates/seiran-common/src/ap/client.rs の verify_signature/build_signing_string）は
// 正規のHTTP Signatures（RSA-SHA256、Digestヘッダー必須）を要求するため、フォロー送信時も
// 同じ規約で実署名する。受信（inbox）側はseiranが送ってくる署名の検証はせず、ただ記録する。

import { createServer, type Server } from "node:http";
import { createHash, createSign, generateKeyPairSync } from "node:crypto";
import { LOCAL_DOMAIN } from "../ports.ts";

// ローカルアクターの正規 AP actor URI（`https://{LOCAL_DOMAIN}/users/{username}`）を組み立てる。
// `seiranBaseUrl`（`http://localhost:PORT`、実際のPOST送信先）をそのまま使うと、seiran側の
// `seiran_common::ap::extract_local_username` が要求するスキーム・ホストと一致せず、
// フォロー・メンション・DM宛先解決が自ドメイン非該当として黙って弾かれる
// （実際のリモートサーバーはWebFinger等で得た正規URIを使うのであって、
// こちらが最初にPOSTしたホスト:ポートをそのまま使い回すことはない）。
function localActorUri(username: string): string {
  return `https://${LOCAL_DOMAIN}/users/${username}`;
}

export interface StubFediServer {
  url: string;
  actorUri: string;
  inboxUrl: string;
  /** 受信した生アクティビティ（Accept・Create・Announce等）を新着順に返す。 */
  receivedActivities(): Record<string, unknown>[];
  /** このスタブアクターからseiranへ署名付きFollowを送り、対象ローカルユーザーをフォローする。 */
  sendFollow(seiranBaseUrl: string, targetUsername: string): Promise<void>;
  /**
   * seiranユーザーから受信したFollow（`receivedActivities()`で取得したもの）に対し、
   * このスタブアクターからAcceptを送り返す（実サーバーの承認挙動を模擬）。
   * Follow の `actor` フィールド（`https://{local_domain}/users/...`）は配送先ホスト:ポートと
   * 一致するとは限らない（`local_domain` はポート番号を含まない識別子）ため、送信先は明示的に渡す。
   */
  sendAccept(seiranBaseUrl: string, followActivity: Record<string, unknown>): Promise<void>;
  /** 対象ローカルユーザー宛のCreate(Note)を送る（`to`に対象アクターのみを指定するとdirect扱い）。
   * `opts.inReplyTo`を指定すると、そのAP Note IDへの返信として送信する。
   * `opts.mentionTargetUsername`を指定すると、そのローカルユーザーへの`tag[].type=="Mention"`
   * （メンション通知E2E用）を`object.tag`に含める。
   * `opts.emojiShortcode`を指定すると、リモート絵文字カタログE2E用のEmoji tagを含める。
   * 返り値は生成したNote ID。 */
  sendCreateNote(
    seiranBaseUrl: string,
    targetUsername: string,
    text: string,
    opts?: {
      inReplyTo?: string;
      mentionTargetUsername?: string;
      emojiShortcode?: string;
      /** "public" 指定時は to に as:Public を含め、cc に対象アクターを置く（Mastodon方式）。省略時は従来どおり to に対象アクターのみ（direct扱い）。 */
      visibility?: "public" | "direct";
      /** ActivityPub summary（CW）。 */
      summary?: string;
      /** アンケート（Question）として送る場合の選択肢名一覧。指定時は object.type が "Question" になる。 */
      pollOptions?: string[];
    },
  ): Promise<string>;
  /** このスタブアクターが送った投稿（`sendCreateNote`が返した Note ID）に対する
   * Delete(Tombstone)をseiranへ送る（リモート削除反映のE2E用）。 */
  sendDeleteNote(seiranBaseUrl: string, noteId: string): Promise<void>;
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
        let activity: Record<string, unknown> | undefined;
        try {
          activity = JSON.parse(raw);
          received.unshift(activity!);
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
          const targetActorUri = localActorUri(targetUsername);
          const activity = {
            "@context": "https://www.w3.org/ns/activitystreams",
            type: "Follow",
            id: `${base}/follows/${Date.now()}`,
            actor: stub.actorUri,
            object: targetActorUri,
          };
          await signedPost(`${seiranBaseUrl}/inbox`, activity, stub.actorUri, privateKey);
        },
        async sendAccept(seiranBaseUrl, followActivity) {
          const acceptActivity = {
            "@context": "https://www.w3.org/ns/activitystreams",
            type: "Accept",
            id: `${base}/accepts/${Date.now()}`,
            actor: stub.actorUri,
            object: followActivity,
          };
          await signedPost(`${seiranBaseUrl}/inbox`, acceptActivity, stub.actorUri, privateKey);
        },
        async sendCreateNote(seiranBaseUrl, targetUsername, text, opts) {
          const targetActorUri = localActorUri(targetUsername);
          const noteId = `${base}/notes/${Date.now()}-${Math.random().toString(36).slice(2)}`;
          const tag: Record<string, unknown>[] = [];
          if (opts?.mentionTargetUsername) {
            tag.push({
              type: "Mention",
              href: localActorUri(opts.mentionTargetUsername),
              name: `@${opts.mentionTargetUsername}`,
            });
          }
          if (opts?.emojiShortcode) {
            tag.push({
              type: "Emoji",
              name: `:${opts.emojiShortcode}:`,
              icon: {
                type: "Image",
                mediaType: "image/png",
                url: `${base}/emoji/${opts.emojiShortcode}.png`,
              },
              aliases: [`alias_${opts.emojiShortcode}`],
              _misskey_license: { freeText: "CC BY 4.0 E2E" },
            });
          }
          const isPublic = opts?.visibility === "public";
          const to = isPublic ? ["https://www.w3.org/ns/activitystreams#Public"] : [targetActorUri];
          const cc = isPublic ? [targetActorUri] : [];
          const poll = opts?.pollOptions
            ? { oneOf: opts.pollOptions.map((name) => ({ type: "Note", name, replies: { type: "Collection", totalItems: 0 } })) }
            : {};
          const activity = {
            "@context": "https://www.w3.org/ns/activitystreams",
            type: "Create",
            id: `${base}/activities/${Date.now()}-${Math.random().toString(36).slice(2)}`,
            actor: stub.actorUri,
            to,
            cc,
            object: {
              type: opts?.pollOptions ? "Question" : "Note",
              id: noteId,
              attributedTo: stub.actorUri,
              content: `<p>${text}</p>`,
              published: new Date().toISOString(),
              to,
              cc,
              tag,
              ...(opts?.inReplyTo ? { inReplyTo: opts.inReplyTo } : {}),
              ...(opts?.summary ? { summary: opts.summary } : {}),
              ...poll,
            },
          };
          await signedPost(`${seiranBaseUrl}/inbox`, activity, stub.actorUri, privateKey);
          return noteId;
        },
        async sendDeleteNote(seiranBaseUrl, noteId) {
          const activity = {
            "@context": "https://www.w3.org/ns/activitystreams",
            type: "Delete",
            id: `${base}/activities/${Date.now()}-${Math.random().toString(36).slice(2)}`,
            actor: stub.actorUri,
            object: { type: "Tombstone", id: noteId },
          };
          await signedPost(`${seiranBaseUrl}/inbox`, activity, stub.actorUri, privateKey);
        },
        close: () => new Promise((res, rej) => server.close((err) => (err ? rej(err) : res()))),
      };
      resolve(stub);
    });
  });
}
