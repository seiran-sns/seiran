use super::*;


/// value（activity/note）の `tag` 配列から、指定した shortcode（`:name:` 形式）に対応する
/// カスタム絵文字タグの画像 URL を取り出す（`build_emoji_map` を利用）。
pub(super) fn extract_emoji_tag_url(value: &serde_json::Value, shortcode: &str) -> Option<String> {
    let tags = value["tag"].as_array().cloned().unwrap_or_default();
    build_emoji_map(&tags)
        .get(shortcode)?
        .as_str()
        .map(|s| s.to_string())
}

/// AP Note の `tag` 配列由来の emoji_map を構築したうえで、本文中に現れる
/// `:shortcode:` のうち tag に含まれていないものを、同一ドメインの `remote_emojis`
/// カタログ（過去の受信で記録済みの絵文字）から補完する（#126）。送信元実装が
/// リノート・編集後の再配送等で `tag` 配列を省略/欠落させても、以前に同じ
/// ドメインから見たことのある絵文字であれば解決できるようにするフォールバック。
pub(super) async fn resolve_emoji_map_with_fallback(
    inbox: &InboxContext,
    domain: &str,
    tags: &[serde_json::Value],
    body: &str,
) -> serde_json::Value {
    let mut map = build_emoji_map(tags);
    let missing: Vec<String> = extract_shortcode_candidates(body)
        .into_iter()
        .filter(|code| map.get(format!(":{}:", code)).is_none())
        .collect();
    if missing.is_empty() {
        return map;
    }
    match inbox
        .remote_emoji_repo
        .find_urls_by_shortcodes(domain, &missing)
        .await
    {
        Ok(pairs) => {
            let obj = map
                .as_object_mut()
                .expect("build_emoji_map always returns an object");
            for (code, url) in pairs {
                obj.insert(format!(":{}:", code), serde_json::Value::String(url));
            }
        }
        Err(e) => {
            tracing::warn!(
                "[RemoteEmoji] 本文フォールバック解決失敗 domain={}: {}",
                domain,
                e
            );
        }
    }
    map
}

pub(super) fn has_unresolved_emoji_shortcodes(tags: &[serde_json::Value], body: &str) -> bool {
    let map = build_emoji_map(tags);
    extract_shortcode_candidates(body)
        .into_iter()
        .any(|code| map.get(format!(":{code}:")).is_none())
}

pub(super) fn has_same_origin(left: &str, right: &str) -> bool {
    let (Ok(left), Ok(right)) = (url::Url::parse(left), url::Url::parse(right)) else {
        return false;
    };
    left.origin() == right.origin()
}

#[cfg(test)]
mod emoji_tag_fallback_tests {
    use super::{has_same_origin, has_unresolved_emoji_shortcodes};

    #[test]
    fn detects_shortcode_missing_from_embedded_note_tags() {
        assert!(has_unresolved_emoji_shortcodes(
            &[],
            "暑くて\u{200b}:tokeru:\u{200b}どころか蒸発する",
        ));
    }

    #[test]
    fn does_not_fetch_when_every_shortcode_has_an_emoji_tag() {
        let tags = vec![serde_json::json!({
            "type": "Emoji",
            "name": ":tokeru:",
            "icon": { "url": "https://example.com/tokeru.png" }
        })];
        assert!(!has_unresolved_emoji_shortcodes(
            &tags,
            "暑くて\u{200b}:tokeru:\u{200b}どころか蒸発する",
        ));
    }

    #[test]
    fn ignores_plain_colon_text_without_a_shortcode() {
        assert!(!has_unresolved_emoji_shortcodes(
            &[],
            "時刻は12:34です https://example.com/a:b",
        ));
    }

    #[test]
    fn canonical_note_fetch_is_limited_to_actor_origin() {
        assert!(has_same_origin(
            "https://misskey.example/notes/1",
            "https://misskey.example/users/alice",
        ));
        assert!(!has_same_origin(
            "http://127.0.0.1/internal",
            "https://misskey.example/users/alice",
        ));
    }
}

/// APのEmoji tagを `remote_emojis` へ記録する（#73）。
/// 投稿本文・表示名・絵文字リアクションのいずれの受信経路からも同じ形で呼ばれる。
/// カタログ記録の失敗は本処理（投稿保存等）を止めるべきではないため、ログのみに留める。
pub(super) async fn record_remote_emojis(inbox: &InboxContext, domain: &str, tags: &[serde_json::Value]) {
    for tag in tags {
        if tag["type"].as_str() != Some("Emoji") {
            continue;
        }
        let Some(name) = tag["name"].as_str() else {
            continue;
        };
        let Some(url) = tag["icon"]["url"].as_str() else {
            continue;
        };
        let shortcode = name.trim_matches(':');
        if shortcode.is_empty() {
            continue;
        }
        // Misskeyはライセンスを `_misskey_license.freeText` で配送する。他実装が
        // aliases/tags/keywordsを添える場合も、既知情報として検索・初期値に利用する。
        let license = tag["_misskey_license"]["freeText"].as_str();
        let remote_tags: Vec<String> = ["aliases", "tags", "keywords"]
            .iter()
            .filter_map(|key| tag[*key].as_array())
            .flatten()
            .filter_map(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if let Err(e) = inbox
            .remote_emoji_repo
            .upsert_seen(shortcode, domain, url, &remote_tags, license)
            .await
        {
            tracing::warn!(
                "[RemoteEmoji] 記録失敗 shortcode={} domain={}: {}",
                shortcode,
                domain,
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_emoji_tag_url_finds_matching_custom_emoji() {
        let activity = serde_json::json!({
            "type": "Like",
            "content": ":blobcat:",
            "_misskey_reaction": ":blobcat:",
            "tag": [
                {
                    "id": "https://misskey.example/emojis/blobcat",
                    "type": "Emoji",
                    "name": ":blobcat:",
                    "icon": { "type": "Image", "mediaType": "image/png", "url": "https://misskey.example/files/blobcat.png" }
                }
            ]
        });
        assert_eq!(
            extract_emoji_tag_url(&activity, ":blobcat:"),
            Some("https://misskey.example/files/blobcat.png".to_string())
        );
    }

    #[test]
    fn extract_emoji_tag_url_ignores_non_matching_name() {
        let activity = serde_json::json!({
            "tag": [
                { "type": "Emoji", "name": ":other:", "icon": { "url": "https://example.com/other.png" } }
            ]
        });
        assert_eq!(extract_emoji_tag_url(&activity, ":blobcat:"), None);
    }

    #[test]
    fn extract_emoji_tag_url_ignores_non_emoji_tag_type() {
        let activity = serde_json::json!({
            "tag": [
                { "type": "Mention", "name": ":blobcat:", "icon": { "url": "https://example.com/x.png" } }
            ]
        });
        assert_eq!(extract_emoji_tag_url(&activity, ":blobcat:"), None);
    }

    #[test]
    fn extract_emoji_tag_url_no_tag_field() {
        let activity = serde_json::json!({ "content": "👍" });
        assert_eq!(extract_emoji_tag_url(&activity, "👍"), None);
    }

    #[test]
    fn extract_emoji_tag_url_unicode_emoji_content_has_no_tag_match() {
        // Unicode 絵文字は通常 tag 配列に一致が無いため None のままになる
        let activity = serde_json::json!({
            "content": "🎉",
            "tag": [
                { "type": "Emoji", "name": ":blobcat:", "icon": { "url": "https://example.com/blobcat.png" } }
            ]
        });
        assert_eq!(extract_emoji_tag_url(&activity, "🎉"), None);
    }
}
