//! notes ハンドラの DTO（リクエスト/レスポンス型）と、DB 行 → レスポンスの素朴な変換。
//! DB アクセスを伴う集約ロジックは `super::queries` を参照。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use seiran_common::repository::TimelinePost;

#[derive(Deserialize)]
pub struct CreateNoteRequest {
    pub text: Option<String>,
    // JS の Number 精度問題を避けるため文字列で受け取り、サーバー側で i64 にパースする
    pub attachment_ids: Option<Vec<String>>,
    pub deliver_to_fedi: Option<bool>,
    pub deliver_to_bsky: Option<bool>,
    /// リポスト元のポスト ID（指定時はリポスト投稿として処理）
    pub renote_id: Option<String>,
    /// リプライ先のポスト ID（指定時はリプライとして処理し配信先を制御する）
    pub reply_to_id: Option<String>,
    /// 引用元のポスト ID（指定時は引用投稿として処理する）
    pub quote_of_id: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentResponse {
    pub url: String,
    pub mime_type: String,
    pub width: i32,
    pub height: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
}

/// ポストに対するリアクション集計（絵文字ごとの件数）(#22)。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReactionSummary {
    pub emoji: String,
    pub count: i64,
    pub reacted_by_me: bool,
    /// Fedi から受信したカスタム絵文字（`:shortcode:`）の画像 URL。Unicode 絵文字は `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji_url: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NoteResponse {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub user: NoteUserInfo,
    pub attachments: Vec<AttachmentResponse>,
    // 7.2 拡張メタデータ
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renote_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_original_id: Option<String>,
    // リアクション集計（#22）。空なら省略。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<ReactionSummary>,
    /// リポスト（renote_id を持つ）の場合の元ポスト実体（#45）。
    /// このノート自身は「リポストした」というラッパで、表示すべき中身は `renote` 側。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renote: Option<Box<NoteResponse>>,
    /// 認証ユーザーがこのノートをリポスト済みかどうか。未認証時は省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reposted_by_me: Option<bool>,
    /// 本文・投稿者表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（Fedi受信のみ、
    /// `posts.emoji_map` と投稿者 `actors.emoji_map` の統合）。フロントは本文・表示名描画時に
    /// このマップで `:shortcode:` を画像に置換する。空なら省略。
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub emojis: HashMap<String, String>,
    /// 認証ユーザー自身の投稿がピン留め済みかどうか（#61）。自分のプロフィール表示時のみ設定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_by_me: Option<bool>,
}

/// `serde_json::Value`（JSONB由来のオブジェクト、`None`/非オブジェクトなら空）を
/// `HashMap<String, String>` に変換する。カスタム絵文字マップ（shortcode→画像URL）の
/// デコードに使う。
fn json_map_to_string_map(v: Option<serde_json::Value>) -> HashMap<String, String> {
    v.and_then(|v| v.as_object().cloned())
        .map(|obj| {
            obj.into_iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NoteUserInfo {
    pub id: i64,
    pub username: String,
    pub domain: Option<String>,
    pub display_name: Option<String>,
    pub actor_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

pub fn to_note_response(p: TimelinePost, attachments: Vec<AttachmentResponse>) -> NoteResponse {
    let mut emojis = json_map_to_string_map(p.post_emoji_map);
    emojis.extend(json_map_to_string_map(p.actor_emoji_map));

    NoteResponse {
        id: p.id.to_string(),
        text: p.body,
        created_at: p.created_at.to_rfc3339(),
        user: NoteUserInfo {
            id: p.actor_id,
            username: p.username,
            domain: Some(p.domain),
            display_name: p.display_name,
            actor_type: if p.actor_type.is_empty() { "local".to_string() } else { p.actor_type },
            avatar_url: p.avatar_url,
        },
        attachments,
        renote_id: p.repost_of_post_id.map(|i| i.to_string()),
        quote_id: p.quote_of_post_id.map(|i| i.to_string()),
        reply_id: p.reply_to_post_id.map(|i| i.to_string()),
        parent_original_id: p.parent_original_post_id.map(|i| i.to_string()),
        reactions: Vec::new(),
        renote: None,
        reposted_by_me: None,
        emojis,
        pinned_by_me: None,
    }
}

#[derive(Deserialize)]
pub struct TimelineQuery {
    pub limit: Option<i64>,
    #[serde(alias = "untilId")]
    pub until_id: Option<String>,
    #[serde(alias = "sinceId")]
    pub since_id: Option<String>,
}

#[derive(Serialize)]
pub struct NoteContextResponse {
    pub before: Vec<NoteResponse>,
    pub after: Vec<NoteResponse>,
}

#[derive(Deserialize)]
pub struct ReactRequest {
    pub content: String,
}
