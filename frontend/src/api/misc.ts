import { request, uploadFormData } from "./core";
import type { AppTokenRow, CreateAppTokenResponse, DriveFile, MetaResponse, PublicEmoji } from "./types";

export function openTarget(target: string) {
  return request<{ path: string; kind: "actor" | "post" }>("POST", "/open", {
    target,
  });
}

export const reports = {
  create(body: {
    subject_type: "actor" | "post";
    subject_actor_id: string;
    subject_post_id?: string;
    reason_type: string;
    reason_text: string;
  }) {
    return request<{ id: string }>("POST", "/reports", body);
  },
};

export function meta(signal?: AbortSignal) {
  return request<MetaResponse>("POST", "/meta", undefined, signal);
}

export const appTokens = {
  /** 設定画面の発行済みアプリトークン一覧（#60）。 */
  list() {
    return request<AppTokenRow[]>("GET", "/account/app-tokens");
  },
  /** 設定画面から直接アプリトークンを発行する（MiAuth連携を介さない）。 */
  create(name?: string) {
    return request<CreateAppTokenResponse>("POST", "/account/app-tokens", { name });
  },
  /** 本人所有のトークンを無効化する（#60）。 */
  revoke(id: string) {
    return request<void>(
      "DELETE",
      `/account/app-tokens/${encodeURIComponent(id)}`,
    );
  },
};

export const media = {
  /**
   * `deliverToBsky`: 動画添付のみ意味を持つ。Bluesky公式動画パイプラインへの
   * 提出可否（省略時true）。falseにすると音声・画像と同様、Bskyへは
   * externalリンクカードとして配信される。
   */
  upload(
    file: File,
    mediaType: "post" | "emoji" | "avatar" | "banner" = "post",
    deliverToBsky = true,
  ): Promise<DriveFile> {
    const formData = new FormData();
    formData.append("file", file);
    formData.append("media_type", mediaType);
    formData.append("deliver_to_bsky", String(deliverToBsky));
    return uploadFormData<DriveFile>("/drive/files/create", formData);
  },
};

export const emojis = {
  /** 公開カスタム絵文字一覧（未認証でも呼べる、Misskey互換 `GET /api/emojis`）。 */
  list() {
    return request<{ emojis: PublicEmoji[] }>("GET", "/emojis");
  },
};
