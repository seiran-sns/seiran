//! Misskey 本家の `Note`/`UserLite`/`UserDetailed` に合わせたレスポンス型。
//!
//! `handlers::notes::NoteResponse` 等の既存カスタム型とは別物。フィールド名はすべて
//! Misskey 本家スキーマの camelCase に合わせる。現状は全フィールドの完全再現ではなく、
//! クライアントの基本描画（タイムライン・リアクション・フォロー）に必要な部分集合。

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyUserLite {
    pub id: String,
    pub username: String,
    /// ローカルユーザーは `null`。
    pub host: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_bot: bool,
    pub is_cat: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyUserDetailed {
    #[serde(flatten)]
    pub lite: MisskeyUserLite,
    pub created_at: String,
    pub description: Option<String>,
    pub banner_url: Option<String>,
    pub is_locked: bool,
    pub is_silenced: bool,
    pub is_suspended: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyDriveFile {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub file_type: String,
    pub url: String,
    pub thumbnail_url: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyNote {
    pub id: String,
    pub created_at: String,
    pub text: Option<String>,
    /// Content Warning。seiran は CW 未対応のため常に `null`。
    pub cw: Option<String>,
    pub user_id: String,
    pub user: MisskeyUserLite,
    pub reply_id: Option<String>,
    /// Misskey は「リノート」と「引用」を区別せず、どちらも `renoteId` + (引用なら)非空の
    /// `text` として表現する。seiran 内部の `repost_of_post_id`（リポスト）と
    /// `quote_of_post_id`（引用）はどちらもここに統合する。
    pub renote_id: Option<String>,
    /// Misskey は「公開範囲」の概念を持つが、seiran は現状すべて公開投稿のみのため固定値。
    pub visibility: String,
    pub file_ids: Vec<String>,
    pub files: Vec<MisskeyDriveFile>,
    pub tags: Vec<String>,
    /// 本文中のカスタム絵文字インライン表示用（shortcode→url）。seiran は未対応のため常に空。
    pub emojis: BTreeMap<String, String>,
    /// 絵文字 → 件数。
    pub reactions: BTreeMap<String, i64>,
    pub renote_count: i64,
    pub replies_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 認証ユーザーが付けたリアクション（絵文字）。未認証・未リアクション時は `null`。
    pub my_reaction: Option<String>,
}
