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
    /// 投稿の可視性（"public" | "unlisted" | "followers_only" | "direct"）。省略時は "public"。
    /// "direct"（DM）指定時は`recipient_actor_ids`が必須。
    pub visibility: Option<String>,
    /// DM（`visibility: "direct"`）の宛先アクターID一覧。JSの Number 精度問題を避けるため
    /// 文字列で受け取る。Misskey本家の`visibleUserIds`と同じ役割のため、そのフィールド名も
    /// エイリアスとして受け付ける（Misskey APIクライアントがBsky DMを読み書きできるようにする
    /// ための互換対応）。
    #[serde(alias = "visibleUserIds")]
    pub recipient_actor_ids: Option<Vec<String>>,
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
    /// Misskey 互換 API（`MisskeyDriveFile`）用。自ドメインアップロードのみ値を持つ
    /// （リモート添付は `media_files` に対応行が無いため `None`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_created_at: Option<String>,
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
    /// 可視性（`unlisted`/`followers_only`/`direct`）。ローカル投稿は投稿作成時の選択、
    /// Fedi受信ポストは`to`/`cc`から判定した値。`public`（デフォルト・大多数のケース）は省略する。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    /// ローカル投稿がFedi/Bskyへ実際に配送されたか（投稿作成時の配送先選択の永続化）。
    /// ローカル投稿以外（リモート受信・リポストラッパー）では省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliver_fedi: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliver_bsky: Option<bool>,
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

/// `mention_facets`（`[{"byteStart":N,"byteEnd":M,"did":"did:plc:..."}]`）を使い、`body` 中の
/// 該当バイト範囲を解決済みハンドル文字列（`@handle` または `@handle@domain`、先頭の `@` 込み）
/// へ置換する。`mention_paths` に無い（未解決）DIDはそのまま変更しない（投稿時点の表示を維持）。
/// フロントの MFM 描画コンポーネントが `@user@host` パターンを検出してプロフィールリンクに
/// 変換する前提のため、ここでは Markdown リンクで包まずプレーンテキストのまま返す。
///
/// `byteStart`/`byteEnd` は保存時点で妥当性検証済みのはずだが、念のため範囲外・非文字境界・
/// 他 facet との重なりはスキップする（表示を壊さない）。
pub fn apply_mention_facets(
    body: &str,
    mention_facets: Option<&serde_json::Value>,
    mention_paths: &HashMap<String, String>,
) -> String {
    let Some(facets) = mention_facets.and_then(|v| v.as_array()) else {
        return body.to_string();
    };
    if facets.is_empty() {
        return body.to_string();
    }

    let mut spans: Vec<(usize, usize, String)> = facets
        .iter()
        .filter_map(|f| {
            let start = f.get("byteStart")?.as_u64()? as usize;
            let end = f.get("byteEnd")?.as_u64()? as usize;
            let did = f.get("did")?.as_str()?.to_string();
            Some((start, end, did))
        })
        .collect();
    // 後ろの facet から順に置換する（前方のオフセットを壊さないため）。
    spans.sort_by_key(|s| std::cmp::Reverse(s.0));

    let mut result = body.to_string();
    let mut upper_bound = result.len();
    for (start, end, did) in spans {
        if start >= end || end > result.len() || end > upper_bound {
            continue;
        }
        if !result.is_char_boundary(start) || !result.is_char_boundary(end) {
            continue;
        }
        if let Some(handle) = mention_paths.get(&did) {
            result.replace_range(start..end, handle);
            upper_bound = start;
        }
        // 未解決なら upper_bound は更新しない（この facet の範囲は変更していないため）。
    }
    result
}

pub fn to_note_response(p: TimelinePost, attachments: Vec<AttachmentResponse>) -> NoteResponse {
    let mut emojis = json_map_to_string_map(p.post_emoji_map);
    emojis.extend(json_map_to_string_map(p.actor_emoji_map));

    let actor_type = if p.actor_type.is_empty() { "local".to_string() } else { p.actor_type };
    let is_local = actor_type == "local";

    NoteResponse {
        id: p.id.to_string(),
        text: p.body,
        created_at: p.created_at.to_rfc3339(),
        user: NoteUserInfo {
            id: p.actor_id,
            username: p.username,
            domain: Some(p.domain),
            display_name: p.display_name,
            actor_type,
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
        visibility: if p.visibility == "public" { None } else { Some(p.visibility) },
        deliver_fedi: if is_local { Some(p.deliver_fedi) } else { None },
        deliver_bsky: if is_local { Some(p.deliver_bsky) } else { None },
    }
}

#[derive(Deserialize)]
pub struct TimelineQuery {
    pub limit: Option<i64>,
    #[serde(alias = "untilId")]
    pub until_id: Option<String>,
    #[serde(alias = "sinceId")]
    pub since_id: Option<String>,
    /// `true`の場合、自分が宛先の`direct`投稿も含め`direct`を一切タイムラインに含めない。
    /// Misskey API互換のためデフォルト`false`（省略時は自分宛のdirectも含まれる）。
    /// seiranフロントエンドはDM専用画面と区別するため常にこれを付与する。
    #[serde(alias = "excludeDirect", default)]
    pub exclude_direct: bool,
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

#[cfg(test)]
mod tests {
    use super::apply_mention_facets;
    use std::collections::HashMap;

    fn facets(spans: &[(usize, usize, &str)]) -> serde_json::Value {
        serde_json::json!(spans
            .iter()
            .map(|(start, end, did)| serde_json::json!({
                "byteStart": start,
                "byteEnd": end,
                "did": did,
            }))
            .collect::<Vec<_>>())
    }

    #[test]
    fn resolved_did_is_replaced_with_handle() {
        let body = "hi @alice.bsky.social!";
        let byte_start = body.find("@alice.bsky.social").unwrap();
        let byte_end = byte_start + "@alice.bsky.social".len();
        let mention_facets = facets(&[(byte_start, byte_end, "did:plc:alice")]);
        let mut mention_paths = HashMap::new();
        mention_paths.insert("did:plc:alice".to_string(), "@alice.bsky.social".to_string());

        let result = apply_mention_facets(body, Some(&mention_facets), &mention_paths);
        assert_eq!(result, "hi @alice.bsky.social!");
    }

    #[test]
    fn handle_change_is_reflected() {
        // 投稿時点は @old.bsky.social だったが、その後ハンドルが変更された想定。
        let body = "hi @old.bsky.social!";
        let byte_start = body.find("@old.bsky.social").unwrap();
        let byte_end = byte_start + "@old.bsky.social".len();
        let mention_facets = facets(&[(byte_start, byte_end, "did:plc:alice")]);
        let mut mention_paths = HashMap::new();
        mention_paths.insert("did:plc:alice".to_string(), "@new.bsky.social".to_string());

        let result = apply_mention_facets(body, Some(&mention_facets), &mention_paths);
        assert_eq!(result, "hi @new.bsky.social!");
    }

    #[test]
    fn unresolved_did_keeps_original_text() {
        let body = "hi @unknown.bsky.social!";
        let byte_start = body.find("@unknown.bsky.social").unwrap();
        let byte_end = byte_start + "@unknown.bsky.social".len();
        let mention_facets = facets(&[(byte_start, byte_end, "did:plc:unknown")]);
        let mention_paths = HashMap::new(); // 未解決

        let result = apply_mention_facets(body, Some(&mention_facets), &mention_paths);
        assert_eq!(result, body, "未解決のDIDは元テキストのまま");
    }

    #[test]
    fn no_mention_facets_returns_body_unchanged() {
        let body = "plain text";
        assert_eq!(apply_mention_facets(body, None, &HashMap::new()), body);
        assert_eq!(
            apply_mention_facets(body, Some(&serde_json::json!([])), &HashMap::new()),
            body
        );
    }

    #[test]
    fn out_of_range_facet_is_skipped_without_panicking() {
        let body = "short";
        let mention_facets = facets(&[(0, 1000, "did:plc:x")]);
        let mut mention_paths = HashMap::new();
        mention_paths.insert("did:plc:x".to_string(), "@x.bsky.social".to_string());
        assert_eq!(apply_mention_facets(body, Some(&mention_facets), &mention_paths), body);
    }

    #[test]
    fn multiple_facets_applied_back_to_front() {
        let body = "@alice and @bob";
        let alice_start = 0;
        let alice_end = "@alice".len();
        let bob_start = body.find("@bob").unwrap();
        let bob_end = bob_start + "@bob".len();
        let mention_facets = facets(&[
            (alice_start, alice_end, "did:plc:alice"),
            (bob_start, bob_end, "did:plc:bob"),
        ]);
        let mut mention_paths = HashMap::new();
        mention_paths.insert("did:plc:alice".to_string(), "@alice.bsky.social".to_string());
        mention_paths.insert("did:plc:bob".to_string(), "@bob.bsky.social".to_string());

        let result = apply_mention_facets(body, Some(&mention_facets), &mention_paths);
        assert_eq!(result, "@alice.bsky.social and @bob.bsky.social");
    }
}
