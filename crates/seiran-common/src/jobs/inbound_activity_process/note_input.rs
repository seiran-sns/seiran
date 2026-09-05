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
pub(super) fn detect_loopback_post_id(
    inbox: &InboxContext,
    note_id: &str,
    note_url: &str,
) -> Option<i64> {
    let loopback_prefix = format!("https://{}/notes/", inbox.local_domain);
    [note_url, note_id].iter().find_map(|url| {
        url.strip_prefix(&loopback_prefix)
            .and_then(|id_str| id_str.parse::<i64>().ok())
    })
}

/// 受信した Note の重複排除（フェーズ5）判定: ブリッジ重複（シナリオ3、note.url が bsky.app
/// の場合に at_uri で既存ポストを探す）を検知し、既存のオリジナル投稿 ID があれば返す。
/// ループバック（シナリオ1）は [`detect_loopback_post_id`] で別途・事前に弾くこと。
pub(super) async fn resolve_bridge_duplicate_post_id(
    inbox: &InboxContext,
    note_url: &str,
) -> Option<i64> {
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
pub(super) fn extract_ap_quote_uri(
    note: &serde_json::Value,
    tags: &[serde_json::Value],
) -> Option<String> {
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
    let is_fallback =
        starts_with_quote_marker(last_line) && quote_uri_matches(last_line, quote_uri);
    if is_fallback {
        trimmed[..last_line_start].trim_end().to_string()
    } else {
        body.to_string()
    }
}

/// Misskey/Fedibird/kmyblueが引用時に本文へ自動付加する`RE:`/`QT:`（kmyblueは`RE `/`QT `という
/// コロン無し表記のこともある、実例: kb.mu7ou.com）フォールバック行かどうかを行頭マーカーで
/// 判定する。
pub(super) fn starts_with_quote_marker(line: &str) -> bool {
    ["RE:", "QT:", "RE ", "QT "]
        .iter()
        .any(|marker| line.starts_with(marker))
}

/// kmyblueは`RE:`/`QT:`フォールバック行を本文の**先頭**に付ける（Fedibird/Misskeyは末尾、
/// 実例: kblue.10rino.net）。`strip_quote_fallback_line`の先頭版。
pub(super) fn strip_quote_fallback_line_leading(body: &str, quote_uri: &str) -> String {
    let trimmed = body.trim_start();
    let first_line_end = trimmed.find('\n').unwrap_or(trimmed.len());
    let first_line = trimmed[..first_line_end].trim();
    let is_fallback =
        starts_with_quote_marker(first_line) && quote_uri_matches(first_line, quote_uri);
    if is_fallback {
        trimmed[first_line_end..].trim_start().to_string()
    } else {
        body.to_string()
    }
}

/// `text` 内のURLが `quote_uri` と同一投稿を指すか判定する。完全一致に加え、Fedibirdは
/// `quoteUrl`（`https://host/users/{user}/statuses/{id}` という内部URL形式）と、本文末尾の
/// QTフォールバックリンク（`https://host/@{user}/{id}` という表示用URL形式）とで
/// URLの形が異なることがある（実例: #117195910938631045）。ActivityPub仕様には別表記URLを
/// 同一オブジェクトと判定する正規化手続きが無い（WebFingerはアクター発見用でありNote単位の
/// URL正規化には使えない）ため、Mastodon/Fedibird系実装の命名規則（末尾セグメントがstatus ID
/// で共通）に頼ったヒューリスティックとして、ホストと末尾ID（英数字6文字以上）が両方
/// `text` に含まれる場合も同一投稿とみなす。
pub(super) fn quote_uri_matches(text: &str, quote_uri: &str) -> bool {
    if text.contains(quote_uri) {
        return true;
    }
    let host = quote_uri
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .filter(|h| !h.is_empty());
    let id = quote_uri.rsplit('/').next().unwrap_or("");
    let id_ok = id.len() >= 6 && id.chars().all(|c| c.is_ascii_alphanumeric());
    match host {
        Some(host) => id_ok && text.contains(host) && text.contains(id),
        None => false,
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

pub(crate) fn normalize_ap_poll(note: &serde_json::Value) -> Option<serde_json::Value> {
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
    fn strips_fedibird_qt_fallback_when_body_url_is_permalink_form() {
        // Fedibirdのquote_uri（quoteUrl拡張フィールド）はAP object id形式
        // (`/users/{user}/statuses/{id}`)だが、本文末尾のQTフォールバックリンクは
        // 人間可読URL形式(`/@{user}/{id}`)であることがある（実例: #117195910938631045）。
        let quote_uri = "https://fedibird.com/users/asata/statuses/117195892358865036";
        assert_eq!(
            strip_quote_fallback_line(
                "本文\nQT: [https://fedibird.com/@asata/117195892358865036](https://fedibird.com/@asata/117195892358865036) [[参照]](https://fedibird.com/@asata/117195910938900437/references)",
                quote_uri,
            ),
            "本文"
        );
    }

    #[test]
    fn does_not_strip_when_host_differs_even_if_id_matches() {
        // 末尾IDだけが偶然一致してもホストが違えば別投稿とみなし、誤って除去しない。
        let quote_uri = "https://fedibird.com/users/asata/statuses/117195892358865036";
        let body = "本文\nRE: [https://other.example/notes/117195892358865036](https://other.example/notes/117195892358865036)";
        assert_eq!(strip_quote_fallback_line(body, quote_uri), body);
    }

    #[test]
    fn strips_kmyblue_re_fallback_at_leading_line() {
        // kmyblue（kblue.10rino.net等）はMarkdown化後のbodyでも`RE:`/`QT:`行を
        // 本文の**先頭**に残す（実例: #117200189063174718）。
        let quote_uri = "https://kblue.10rino.net/users/mz/statuses/117200184727293348";
        let body = "RE: [https://kblue.10rino.net/@mz/117200184727293348](https://kblue.10rino.net/@mz/117200184727293348)\n\nすみません永久不滅ポイントは永久不滅のままで";
        assert_eq!(
            strip_quote_fallback_line_leading(body, quote_uri),
            "すみません永久不滅ポイントは永久不滅のままで"
        );
    }

    #[test]
    fn strips_kmyblue_colonless_re_fallback_at_tail() {
        // kmyblue（kb.mu7ou.com）の一部投稿は`RE:`ではなくコロン無し`RE `を末尾に使う
        // （実例: #117018482769922445）。quote_uriはAPオブジェクトID形式、本文中は
        // 表示用URL形式。
        let quote_uri =
            "https://kb.mu7ou.com/ap/users/116805310384210610/statuses/117017885539545746";
        let body = "一時的に見えるようにした :bunhdlurk:\n\nRE [https://kb.mu7ou.com/@m/117017885539545746](https://kb.mu7ou.com/@m/117017885539545746)";
        assert_eq!(
            strip_quote_fallback_line(body, quote_uri),
            "一時的に見えるようにした :bunhdlurk:"
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
