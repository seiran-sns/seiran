use super::*;


/// `https://bsky.app/profile/{did}/post/{rkey}` → `at://{did}/app.bsky.feed.post/{rkey}`
pub(super) fn bsky_app_url_to_at_uri(url: &str) -> Option<String> {
    let without_prefix = url.strip_prefix("https://bsky.app/profile/")?;
    let mut parts = without_prefix.splitn(3, '/');
    let did = parts.next()?;
    let post_label = parts.next()?;
    if post_label != "post" {
        return None;
    }
    let rkey = parts.next()?;
    Some(format!("at://{}/app.bsky.feed.post/{}", did, rkey))
}

/// 受信した Note のループバック（シナリオ1: note.id または note.url が自ドメインの notes URL
/// を名乗る）を検知する。配送経路の異常（リレー等が Create の object.id/url を書き換えて送り
/// 返してくる等）で発生し、該当ノートは既にローカルに存在するため、呼び出し元はこれを新規
/// INSERTせず、返ってきた既存 post_id をそのまま使うか活動自体を無視しなければならない
/// （#117022998620934901 で発覚: このガードが無かったため domain はローカルだが id が
/// 一致しない重複行が生成された）。
pub(super) fn detect_loopback_post_id(inbox: &InboxContext, note_id: &str, note_url: &str) -> Option<i64> {
    let loopback_prefix = format!("https://{}/notes/", inbox.local_domain);
    [note_url, note_id].iter().find_map(|url| {
        url.strip_prefix(&loopback_prefix)
            .and_then(|id_str| id_str.parse::<i64>().ok())
    })
}

/// 受信した Note の重複排除（フェーズ5）判定: ブリッジ重複（シナリオ3、note.url が bsky.app
/// の場合に at_uri で既存ポストを探す）を検知し、既存のオリジナル投稿 ID があれば返す。
/// ループバック（シナリオ1）は [`detect_loopback_post_id`] で別途・事前に弾くこと。
pub(super) async fn resolve_bridge_duplicate_post_id(inbox: &InboxContext, note_url: &str) -> Option<i64> {
    let at_uri = bsky_app_url_to_at_uri(note_url)?;
    inbox
        .post_repo
        .find_id_by_at_uri(&at_uri)
        .await
        .ok()
        .flatten()
}

/// AP Note から引用元URIを抽出する（#116）。Fedibirdは `quoteUrl`、Misskeyは `quoteUrl` と
/// `_misskey_quote` の両方を持つ（同一値）。`quoteUrl` が無い実装向けに `_misskey_quote` を
/// フォールバックとして見る。さらにFedibirdは `_misskey_quote` に加え
/// `tag[].rel == "https://misskey-hub.net/ns#_misskey_quote"` にも同じURIを持つ場合があるため、
/// 両フィールドが無ければ最後に `tag` を走査する（`quoteUrl` → `_misskey_quote` → `tag` の順）。
/// 送信側は `ap/deliver.rs` の `build_create_note_activity` が同じ2フィールドを付与している。
pub(super) fn extract_ap_quote_uri(note: &serde_json::Value, tags: &[serde_json::Value]) -> Option<String> {
    note["quoteUrl"]
        .as_str()
        .or_else(|| note["_misskey_quote"].as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            tags.iter().find_map(|tag| {
                if tag["rel"].as_str() == Some("https://misskey-hub.net/ns#_misskey_quote") {
                    tag["href"].as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
        })
}

/// Misskey/Fedibirdは引用時にNote本文末尾へ、`quote_uri` と同じURLを指す
/// `RE: [URL](URL)`（Misskey）または `QT: [URL](URL)`（Fedibird）というプレーンテキストの
/// フォールバックリンクを自動付加する（`ap_content_to_markdown_body` によるMarkdown化後もこの
/// 形で本文に残る）。引用元は既に `quote_of_post_id`/`quote` フィールドとして構造化保存・表示
/// されるため、この重複行を本文末尾から取り除く。`quote_uri` と一致するURLを含む末尾の
/// `RE:`/`QT:` 行のみを対象とし、それ以外の本文（ユーザーが独自に書いた `RE:` 始まりの行等）は
/// 過剰除去しない。
pub(super) fn strip_quote_fallback_line(body: &str, quote_uri: &str) -> String {
    let trimmed = body.trim_end();
    let last_line_start = trimmed.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let last_line = trimmed[last_line_start..].trim();
    let is_fallback = (last_line.starts_with("RE:") || last_line.starts_with("QT:"))
        && last_line.contains(quote_uri);
    if is_fallback {
        trimmed[..last_line_start].trim_end().to_string()
    } else {
        body.to_string()
    }
}

/// AP attachment の実 MIME タイプを判定する。
/// 多くの実装（Mastodon 等）は `mediaType` を明示するのでそれを優先し、
/// 欠けている場合のみ URL の拡張子から推測する（判別不能なら `None`）。
pub(super) fn guess_attachment_mime_type(att: &serde_json::Value, url: &str) -> Option<String> {
    if let Some(mt) = att["mediaType"].as_str() {
        if !mt.is_empty() {
            return Some(mt.to_string());
        }
    }
    let ext = url.rsplit('.').next()?.to_ascii_lowercase();
    let guessed = match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        _ => return None,
    };
    Some(guessed.to_string())
}

pub(super) fn normalize_ap_poll(note: &serde_json::Value) -> Option<serde_json::Value> {
    if note["type"].as_str() != Some("Question") {
        return None;
    }
    let (choices, multiple) = if let Some(v) = note["oneOf"].as_array() {
        (v, false)
    } else {
        (note["anyOf"].as_array()?, true)
    };
    let options: Vec<_> = choices
        .iter()
        .filter_map(|choice| {
            Some(serde_json::json!({
                "name": choice["name"].as_str()?,
                "votes": choice["replies"]["totalItems"].as_i64().unwrap_or(0).max(0)
            }))
        })
        .collect();
    if options.is_empty() {
        return None;
    }
    Some(serde_json::json!({
        "multiple": multiple,
        "options": options,
        "endTime": note["endTime"].as_str(),
        "closed": note["closed"].as_str(),
        "votersCount": note["votersCount"].as_i64(),
    }))
}

/// `tag[]` の `Mention` エントリから、自ドメインのローカルユーザーを指すものだけを
/// username として抽出する（`extract_local_username` でホスト名まで検証するため、
/// 同一usernameを名乗る他インスタンスのアクターへの参照タグは含まれない）。
pub(super) fn extract_mentioned_local_usernames<'a>(
    tags: &'a [serde_json::Value],
    local_domain: &str,
) -> Vec<&'a str> {
    tags.iter()
        .filter(|tag| tag["type"].as_str() == Some("Mention"))
        .filter_map(|tag| tag["href"].as_str())
        .filter_map(|href| crate::ap::extract_local_username(href, local_domain))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_mentioned_local_usernames_ignores_same_name_actor_on_foreign_host() {
        // 本人が複数インスタンスに同名アカウントを持ち、その一つ（他インスタンス）への
        // 自己参照Mentionタグが本文中に見えない形で含まれるケース（WordPress
        // ActivityPubプラグイン等のクロスポストで実際に観測された）。ローカルの
        // 同名ユーザー宛のタグだけが拾われ、他ホストのタグは無視されるべき。
        let tags = vec![
            serde_json::json!({
                "type": "Mention",
                "href": "https://mstdn.jp/users/atasinti"
            }),
            serde_json::json!({
                "type": "Mention",
                "href": "https://seiran-beta.org/users/atasinti"
            }),
        ];
        assert_eq!(
            extract_mentioned_local_usernames(&tags, "seiran-beta.org"),
            vec!["atasinti"]
        );
    }

    #[test]
    fn extract_mentioned_local_usernames_empty_when_only_foreign_host_tags() {
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://fedibird.com/users/momozou"
        })];
        assert!(extract_mentioned_local_usernames(&tags, "seiran-beta.org").is_empty());
    }

    #[test]
    fn extracts_misskey_quote_url_and_strips_re_fallback() {
        let uri = "https://seiran.example/notes/123";
        let note = serde_json::json!({ "quoteUrl": uri, "_misskey_quote": uri });
        assert_eq!(extract_ap_quote_uri(&note, &[]).as_deref(), Some(uri));
        assert_eq!(
            strip_quote_fallback_line(
                "引用ポストのテスト\nRE: [https://seiran.example/notes/123](https://seiran.example/notes/123)",
                uri,
            ),
            "引用ポストのテスト"
        );
    }

    #[test]
    fn extracts_fedibird_quote_tag_and_strips_qt_fallback() {
        let uri = "https://seiran.example/notes/123";
        let note = serde_json::json!({});
        let tags = vec![serde_json::json!({
            "type": "Link",
            "rel": "https://misskey-hub.net/ns#_misskey_quote",
            "href": uri
        })];
        assert_eq!(extract_ap_quote_uri(&note, &tags).as_deref(), Some(uri));
        assert_eq!(
            strip_quote_fallback_line(
                "引用ポストのテスト\nQT: [https://seiran.example/notes/123](https://seiran.example/notes/123)",
                uri,
            ),
            "引用ポストのテスト"
        );
    }

    #[test]
    fn does_not_strip_unrelated_re_line() {
        assert_eq!(
            strip_quote_fallback_line(
                "本文\nRE: [別URL](https://example.com/other)",
                "https://seiran.example/notes/123",
            ),
            "本文\nRE: [別URL](https://example.com/other)"
        );
    }

    #[test]
    fn bsky_app_url_to_at_uri_valid() {
        assert_eq!(
            bsky_app_url_to_at_uri("https://bsky.app/profile/did:plc:abc123/post/xyz789"),
            Some("at://did:plc:abc123/app.bsky.feed.post/xyz789".to_string())
        );
    }

    #[test]
    fn bsky_app_url_to_at_uri_wrong_label() {
        assert_eq!(
            bsky_app_url_to_at_uri("https://bsky.app/profile/did:plc:abc123/likes/xyz789"),
            None
        );
    }

    #[test]
    fn bsky_app_url_to_at_uri_not_bsky_app() {
        assert_eq!(bsky_app_url_to_at_uri("https://example.com/notes/1"), None);
        assert_eq!(bsky_app_url_to_at_uri(""), None);
    }

    #[test]
    fn normalizes_question_poll_without_trusting_negative_counts() {
        let question = serde_json::json!({
            "type": "Question",
            "oneOf": [
                { "name": "紅茶", "replies": { "totalItems": 3 } },
                { "name": "珈琲", "replies": { "totalItems": -2 } }
            ],
            "endTime": "2026-07-28T00:00:00Z",
            "votersCount": 3
        });
        assert_eq!(
            normalize_ap_poll(&question),
            Some(serde_json::json!({
                "multiple": false,
                "options": [
                    { "name": "紅茶", "votes": 3 },
                    { "name": "珈琲", "votes": 0 }
                ],
                "endTime": "2026-07-28T00:00:00Z",
                "closed": null,
                "votersCount": 3
            }))
        );
    }
}
