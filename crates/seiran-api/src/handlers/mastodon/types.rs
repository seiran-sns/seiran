//! Mastodon REST API のエンティティ（https://docs.joinmastodon.org/entities/）。
//!
//! クライアント（Tusky・Ice Cubes・Elk・Phanpy 等）は必須フィールドの欠落や型違いで
//! エンティティ全体のデコードに失敗し、タイムライン1画面ごと表示できなくなることがある。
//! Mastodon 本家が常に出すフィールドは、値が無くても `null`・空配列・空文字で必ず出す
//! （`skip_serializing_if` を使わない）。URL 型のフィールドに空文字を入れると Swift 系
//! クライアントの `URL` デコードが失敗するため、URL は実在する値を入れる。

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct MastodonEmoji {
    pub shortcode: String,
    pub url: String,
    pub static_url: String,
    pub visible_in_picker: bool,
    pub category: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonField {
    pub name: String,
    /// HTML。
    pub value: String,
    pub verified_at: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonAccount {
    pub id: String,
    pub username: String,
    /// ローカルは `username`、リモートは `username@domain`（Mastodon 本家の慣習）。
    pub acct: String,
    pub url: String,
    pub uri: String,
    pub display_name: String,
    /// HTML。
    pub note: String,
    pub avatar: String,
    pub avatar_static: String,
    pub header: String,
    pub header_static: String,
    pub locked: bool,
    pub bot: bool,
    pub group: bool,
    pub discoverable: bool,
    pub indexable: bool,
    pub created_at: String,
    pub last_status_at: Option<String>,
    pub statuses_count: i64,
    pub followers_count: i64,
    pub following_count: i64,
    pub emojis: Vec<MastodonEmoji>,
    pub fields: Vec<MastodonField>,
    /// `verify_credentials`（自分自身）のときだけ出す。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<MastodonAccountSource>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonAccountSource {
    pub privacy: &'static str,
    pub sensitive: bool,
    pub language: Option<String>,
    /// プレーンテキスト（編集画面の初期値）。
    pub note: String,
    pub fields: Vec<MastodonField>,
    pub follow_requests_count: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonMediaMetaSize {
    pub width: i32,
    pub height: i32,
    pub size: String,
    pub aspect: f64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct MastodonMediaMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<MastodonMediaMetaSize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small: Option<MastodonMediaMetaSize>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonMediaAttachment {
    pub id: String,
    /// `image` / `gifv` / `video` / `audio` / `unknown`。
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub url: String,
    pub preview_url: String,
    pub remote_url: Option<String>,
    pub meta: MastodonMediaMeta,
    pub description: Option<String>,
    pub blurhash: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonPollOption {
    pub title: String,
    pub votes_count: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonPoll {
    pub id: String,
    pub expires_at: Option<String>,
    pub expired: bool,
    pub multiple: bool,
    pub votes_count: i64,
    pub voters_count: Option<i64>,
    pub options: Vec<MastodonPollOption>,
    pub emojis: Vec<MastodonEmoji>,
    pub voted: bool,
    pub own_votes: Vec<i32>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonTag {
    pub name: String,
    pub url: String,
    pub history: Vec<serde_json::Value>,
    pub following: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonMention {
    pub id: String,
    pub username: String,
    pub url: String,
    pub acct: String,
}

/// Mastodon 4.5 の引用（`Quote` エンティティ）。seiran は引用の承認制を持たず、見えている
/// 引用は常に `accepted`。
#[derive(Serialize, Clone, Debug)]
pub struct MastodonQuote {
    /// `accepted` / `deleted` / `unauthorized` 等。
    pub state: &'static str,
    pub quoted_status: Option<Box<MastodonStatus>>,
}

/// Mastodon 4.5 の `quote_approval`。クライアントはこれで引用ボタンの可否を決める。
#[derive(Serialize, Clone, Debug)]
pub struct MastodonQuoteApproval {
    pub automatic: Vec<&'static str>,
    pub manual: Vec<&'static str>,
    /// `automatic` / `manual` / `denied` / `unknown`。
    pub current_user: &'static str,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonStatus {
    pub id: String,
    pub uri: String,
    pub url: Option<String>,
    pub created_at: String,
    pub edited_at: Option<String>,
    pub account: MastodonAccount,
    /// HTML。
    pub content: String,
    /// `public` / `unlisted` / `private` / `direct`。
    pub visibility: &'static str,
    pub sensitive: bool,
    pub spoiler_text: String,
    pub media_attachments: Vec<MastodonMediaAttachment>,
    pub application: Option<serde_json::Value>,
    pub mentions: Vec<MastodonMention>,
    pub tags: Vec<MastodonTag>,
    pub emojis: Vec<MastodonEmoji>,
    pub reblogs_count: i64,
    pub favourites_count: i64,
    pub replies_count: i64,
    pub quotes_count: i64,
    pub in_reply_to_id: Option<String>,
    pub in_reply_to_account_id: Option<String>,
    pub reblog: Option<Box<MastodonStatus>>,
    pub quote: Option<MastodonQuote>,
    pub quote_approval: MastodonQuoteApproval,
    pub poll: Option<MastodonPoll>,
    pub card: Option<serde_json::Value>,
    pub language: Option<String>,
    pub text: Option<String>,
    pub filtered: Vec<serde_json::Value>,
    /// 閲覧者依存のフィールド。未ログインでは Mastodon 本家同様に省く。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favourited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reblogged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonContext {
    pub ancestors: Vec<MastodonStatus>,
    pub descendants: Vec<MastodonStatus>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonRelationship {
    pub id: String,
    pub following: bool,
    pub showing_reblogs: bool,
    pub notifying: bool,
    pub languages: Option<Vec<String>>,
    pub followed_by: bool,
    pub blocking: bool,
    pub blocked_by: bool,
    pub muting: bool,
    pub muting_notifications: bool,
    pub requested: bool,
    pub requested_by: bool,
    pub domain_blocking: bool,
    pub endorsed: bool,
    pub note: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonNotification {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub created_at: String,
    pub account: MastodonAccount,
    pub status: Option<MastodonStatus>,
    /// `pleroma:emoji_reaction` のときのリアクション内容（Pleroma/Akkoma 互換クライアント向け）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji_url: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonList {
    pub id: String,
    pub title: String,
    pub replies_policy: &'static str,
    pub exclusive: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonSearchResults {
    pub accounts: Vec<MastodonAccount>,
    pub statuses: Vec<MastodonStatus>,
    pub hashtags: Vec<MastodonTag>,
}

/// `POST /api/v1/apps`・`GET /api/v1/apps/verify_credentials` の応答。
#[derive(Serialize, Clone, Debug)]
pub struct MastodonApplication {
    pub id: String,
    pub name: String,
    pub website: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
    pub redirect_uris: Vec<String>,
    /// 登録直後（`POST /api/v1/apps`）だけ出す。secret はハッシュしか保存しないため、
    /// 後から再表示できない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub vapid_key: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct MastodonToken {
    pub access_token: String,
    pub token_type: &'static str,
    pub scope: String,
    pub created_at: i64,
}
