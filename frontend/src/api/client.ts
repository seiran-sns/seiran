//! API クライアント集約モジュール。
//!
//! - `core`: fetch 共通処理（`request`/`uploadFormData`/エラー変換）
//! - `types`: リクエスト/レスポンス型、`Note` 正規化（`normalizeNote`/`noteFromStream`）
//! - `webauthn`: パスキー（WebAuthn）のchallenge/レスポンス変換ヘルパー
//! - ドメイン別モジュール（`auth`/`notes`/`users`/`admin`/`follows`/`lists`/`misc`）:
//!   各ドメインの `api.xxx` エンドポイント群
//!
//! このファイル自体は、既存の `import { api, ... } from "../api/client"` を
//! 変更せずに済むよう、各ドメインの再エクスポートと `api` オブジェクトの組み立てのみ持つ。

export * from "./core";
export * from "./types";

import { openTarget, reports, meta, appTokens, media, emojis } from "./misc";
import { setup, auth, miauth } from "./auth";
import { notes, notifications, reactions } from "./notes";
import { users, alsoKnownAs, blocks, mutes, actors, account } from "./users";
import { admin } from "./admin";
import { follows, followImport, dm } from "./follows";
import { lists, hashtags } from "./lists";

export const api = {
  openTarget,
  reports,
  meta,
  setup,
  auth,
  notes,
  notifications,
  users,
  alsoKnownAs,
  admin,
  follows,
  followImport,
  blocks,
  mutes,
  actors,
  dm,
  lists,
  hashtags,
  account,
  miauth,
  appTokens,
  media,
  emojis,
  reactions,
};
