/// `at://did/collection/rkey` 形式の AT URI を Bsky.app URL に変換するヘルパー。
/// Fedi フォールバック配送（Bskyネイティブ投稿のリポスト等）で、outbox 表示と
/// push 配送の両方から共通利用する。
pub fn at_uri_to_bsky_app_url(at_uri: &str) -> String {
    let without_prefix = at_uri.strip_prefix("at://").unwrap_or(at_uri);
    let parts: Vec<&str> = without_prefix.splitn(3, '/').collect();
    if parts.len() >= 3 {
        let did = parts[0];
        let rkey = parts[2];
        format!("https://bsky.app/profile/{}/post/{}", did, rkey)
    } else {
        at_uri.to_string()
    }
}

/// HTML の特殊文字をエスケープする（`plain_to_html`／`plain_to_html_with_mentions` 共通）。
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// プレーンテキストを ActivityPub 向け HTML に変換する
///
/// 空行で段落分割し、改行を `<br>` に変換する。
pub fn plain_to_html(text: &str) -> String {
    let paragraphs: Vec<String> = text
        .split("\n\n")
        .map(|para| format!("<p>{}</p>", escape_html(para).replace('\n', "<br>")))
        .collect();
    paragraphs.join("")
}

/// プレーンテキストを ActivityPub 向け HTML に変換する（メンション/リンク span 対応版）。
///
/// `mentions` の `byte_start`/`byte_end`（`text` に対する UTF-8 バイトオフセット）区間を
/// `<a href="...">` に置き換えてから、`plain_to_html` と同じ段落分割・改行変換を行う。
/// `mentions` は `crate::mention::convert_mentions_for_ap` の戻り値をそのまま渡す想定
/// （byte_start 昇順・非重複であること）。
pub fn plain_to_html_with_mentions(
    text: &str,
    mentions: &[crate::mention::ApInlineMention],
) -> String {
    let mut linked = String::with_capacity(text.len() * 2);
    let mut last = 0usize;
    for m in mentions {
        if m.byte_start < last || m.byte_end > text.len() || m.byte_start > m.byte_end {
            // 不正な範囲（呼び出し側のバグ等）はそのスパンだけ無視して安全側に倒す
            continue;
        }
        linked.push_str(&escape_html(&text[last..m.byte_start]));
        let rel = match m.kind {
            crate::mention::ApInlineSpanKind::Mention => {
                r#" class="mention u-url" rel="nofollow noopener""#
            }
            // Mastodon 等が実際に送ってくる形式（`class="mention hashtag" rel="tag"`）に合わせる。
            // 受信側の `ap_content_to_markdown_body` はこの形式のアンカーを `#foo` として
            // 解決できることを確認済み（`docs/protocols.md` 6節・`jobs::inbound_activity_process`
            // のテスト参照）。
            crate::mention::ApInlineSpanKind::Hashtag => r#" class="mention hashtag" rel="tag""#,
            crate::mention::ApInlineSpanKind::Link => r#" rel="nofollow noopener""#,
        };
        linked.push_str(&format!(
            r#"<a href="{}"{}>{}</a>"#,
            escape_html(&m.href),
            rel,
            escape_html(&m.name)
        ));
        last = m.byte_end;
    }
    linked.push_str(&escape_html(&text[last..]));

    let paragraphs: Vec<String> = linked
        .split("\n\n")
        .map(|para| format!("<p>{}</p>", para.replace('\n', "<br>")))
        .collect();
    paragraphs.join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_to_html_single_paragraph() {
        assert_eq!(plain_to_html("Hello"), "<p>Hello</p>");
        assert_eq!(plain_to_html("Hello, world!"), "<p>Hello, world!</p>");
    }

    #[test]
    fn test_plain_to_html_double_newline() {
        assert_eq!(plain_to_html("Hello\n\nWorld"), "<p>Hello</p><p>World</p>");
        assert_eq!(
            plain_to_html("First\n\nSecond\n\nThird"),
            "<p>First</p><p>Second</p><p>Third</p>"
        );
        // 単一改行は <br> になる
        assert_eq!(plain_to_html("line1\nline2"), "<p>line1<br>line2</p>");
    }

    #[test]
    fn test_plain_to_html_no_xss() {
        let result = plain_to_html("<script>alert(1)</script>");
        // <script> タグがそのままHTMLとして出力されないこと
        assert!(!result.contains("<script>"));
        assert!(!result.contains("</script>"));
        assert!(result.contains("&lt;script&gt;"));
        assert_eq!(result, "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>");
    }

    #[test]
    fn plain_to_html_with_mentions_wraps_mention_in_anchor() {
        let text = "hello @alice@seiran.example bye";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 6,
            byte_end: 27,
            href: "https://seiran.example/users/alice".to_string(),
            name: "@alice@seiran.example".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert_eq!(
            html,
            r#"<p>hello <a href="https://seiran.example/users/alice" class="mention u-url" rel="nofollow noopener">@alice@seiran.example</a> bye</p>"#
        );
    }

    #[test]
    fn plain_to_html_with_mentions_non_mention_link_omits_mention_class() {
        let text = "see alice.bsky.social";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 4,
            byte_end: 21,
            href: "https://bsky.app/profile/alice.bsky.social".to_string(),
            name: "alice.bsky.social".to_string(),
            kind: crate::mention::ApInlineSpanKind::Link,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert!(!html.contains("class=\"mention"));
        assert!(html.contains(r#"<a href="https://bsky.app/profile/alice.bsky.social" rel="nofollow noopener">alice.bsky.social</a>"#));
    }

    #[test]
    fn plain_to_html_with_mentions_escapes_surrounding_text() {
        let text = "<b>@alice</b>";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 3,
            byte_end: 9,
            href: "https://seiran.example/users/alice".to_string(),
            name: "@alice".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert!(html.starts_with("<p>&lt;b&gt;<a "));
        assert!(html.ends_with("</a>&lt;/b&gt;</p>"));
    }

    #[test]
    fn plain_to_html_with_mentions_out_of_range_span_is_skipped() {
        let text = "hi";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 0,
            byte_end: 100, // text の範囲外
            href: "https://example.com".to_string(),
            name: "x".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert_eq!(html, "<p>hi</p>");
    }

    #[test]
    fn plain_to_html_with_mentions_preserves_newlines() {
        let text = "@alice\nsecond line";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 0,
            byte_end: 6,
            href: "https://seiran.example/users/alice".to_string(),
            name: "@alice".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert!(html.contains("<br>second line"));
    }
}
