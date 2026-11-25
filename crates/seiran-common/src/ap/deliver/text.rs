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

/// `at://did/collection/rkey` 形式の AT URI から、投稿者の Bsky.app プロフィール URL を作る。
pub fn at_uri_to_bsky_profile_url(at_uri: &str) -> Option<String> {
    let without_prefix = at_uri.strip_prefix("at://")?;
    let did = without_prefix.split('/').next().filter(|d| !d.is_empty())?;
    Some(format!("https://bsky.app/profile/{}", did))
}

/// Bsky にしか実体が無い投稿を引用したとき、Fedi 向け本文の末尾に付ける引用ブロック。
/// URL をカードにしない表示環境でも引用元の内容が読めるようにする。1行目は引用元の投稿者
/// （表示名をプロフィールへのリンクにし、ハンドルを添える）、続けて引用元の本文。
///
/// 受け取る側の言語は分からないので、「〜の投稿」のような文言は付けない。
///
/// Misskey は受信した `<blockquote>` の中身をタグを捨てた文字列にしてから引用にする（残るのは
/// `<br>` の改行だけ）。そのため中は1つの `<p>` にまとめ、行・段落の区切りを `<br>` で書く
/// （`<p>` で区切ると Misskey では1行につながる）。同じ理由で `<code>` も捨てられ、本文に残った
/// `@alice.bsky.social` はメンション構文として読まれて、存在しないメンションになってしまう。
/// ハンドル（1行目と本文中のメンション）は `@` の直後に単語結合子（U+2060、見えない文字）を
/// 挟み、メンションとして成立しないようにする（`<code>` は Mastodon 等で等幅にする見た目だけ）。
/// `@` で始まる文字列を `<a>` の中に置かないのも、Misskey が `@` で始まるリンクをメンションに
/// 変換するため。
/// 本文が空（画像だけの投稿等）なら、何を引用したかはリンクで分かるので付けない。
pub fn bsky_quote_block_html(
    display_name: Option<&str>,
    handle: &str,
    profile_url: &str,
    body: &str,
) -> Option<String> {
    if body.trim().is_empty() {
        return None;
    }
    let name = display_name
        .map(str::trim)
        .filter(|n| !n.is_empty() && *n != handle);
    let handle_code = handle_code_html(handle);
    let author = match name {
        Some(name) => format!(
            r#"<a href="{}" rel="nofollow noopener">{}</a> {}"#,
            escape_html(profile_url),
            escape_html(name),
            handle_code
        ),
        // 表示名が無ければハンドルだけ。ハンドルをリンクにすると `@` で始まるリンクになる
        // ので付けない（プロフィールへは直前の投稿 URL から辿れる）。
        None => handle_code,
    };
    Some(format!(
        "<blockquote><p>{}<br>{}</p></blockquote>",
        author,
        markdown_links_to_html(body.trim())
    ))
}

/// 内部リンクマーカー `[表示テキスト](URL)`（Bsky の `#link` facet を焼き込んだもの、
/// `atp::facets::apply_link_facets`）を `<a>` にし、他は `plain_to_html` と同じく段落・改行を
/// `<br>` にする（段落も `<br><br>`。理由は `bsky_quote_block_html`）。リモート Bsky 投稿の本文を
/// そのまま HTML にするとマーカーが記号のまま見えるため。
/// URL が `http(s)://` で始まらないもの・閉じていないものはテキストのまま残す。
fn markdown_links_to_html(text: &str) -> String {
    let mut linked = String::with_capacity(text.len() * 2);
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after_open = &rest[open + 1..];
        let link = after_open.find("](").and_then(|mid| {
            let label = &after_open[..mid];
            let url_part = &after_open[mid + 2..];
            let close = url_part.find(')')?;
            let url = &url_part[..close];
            let valid = !label.contains(['\n', '[', ']'])
                && (url.starts_with("https://") || url.starts_with("http://"))
                && !url.contains(char::is_whitespace);
            valid.then_some((label, url, open + 1 + mid + 2 + close + 1))
        });
        match link {
            Some((label, url, consumed)) => {
                linked.push_str(&escape_with_handle_code(&rest[..open]));
                linked.push_str(&format!(
                    r#"<a href="{}" rel="nofollow noopener">{}</a>"#,
                    escape_html(url),
                    escape_html(label)
                ));
                rest = &rest[consumed..];
            }
            None => {
                linked.push_str(&escape_with_handle_code(&rest[..=open]));
                rest = &rest[open + 1..];
            }
        }
    }
    linked.push_str(&escape_with_handle_code(rest));
    linked.replace('\n', "<br>")
}

/// 引用ブロック中のハンドル表記（理由は `bsky_quote_block_html`）。
fn handle_code_html(handle: &str) -> String {
    format!("<code>@\u{2060}{}</code>", escape_html(handle))
}

/// HTML エスケープしつつ、本文中の Bsky ハンドルのメンション（`@` の後にドットを含むドメイン
/// 形式の名前）を `<code>` で囲む（理由は `bsky_quote_block_html`）。メールアドレス等の途中の
/// `@`（直前が英数字等）は対象にしない。
fn escape_with_handle_code(text: &str) -> String {
    let is_handle_char = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
    let mut out = String::with_capacity(text.len() + 16);
    let mut plain_start = 0;
    let mut prev: Option<char> = None;
    let mut iter = text.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        let starts_mention = c == '@'
            && !prev.is_some_and(|p| p.is_alphanumeric() || matches!(p, '_' | '.' | '@' | '-'));
        if starts_mention {
            let rest = &text[i + 1..];
            let len = rest
                .find(|ch: char| !is_handle_char(ch))
                .unwrap_or(rest.len());
            let handle = rest[..len].trim_end_matches(['.', '-']);
            let valid =
                handle.contains('.') && !handle.starts_with(['.', '-']) && !handle.contains("..");
            if valid {
                out.push_str(&escape_html(&text[plain_start..i]));
                out.push_str(&handle_code_html(handle));
                let end = i + 1 + handle.len();
                plain_start = end;
                while iter.peek().is_some_and(|(j, _)| *j < end) {
                    iter.next();
                }
                prev = handle.chars().last();
                continue;
            }
        }
        prev = Some(c);
    }
    out.push_str(&escape_html(&text[plain_start..]));
    out
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
    fn bsky_quote_block_html_puts_author_then_body() {
        let html = bsky_quote_block_html(
            Some("Alice <A>"),
            "alice.bsky.social",
            "https://bsky.app/profile/did:plc:abc",
            "line1\nline2\n\npara2",
        )
        .unwrap();
        assert_eq!(
            html,
            "<blockquote><p><a href=\"https://bsky.app/profile/did:plc:abc\" rel=\"nofollow noopener\">Alice &lt;A&gt;</a> <code>@\u{2060}alice.bsky.social</code><br>line1<br>line2<br><br>para2</p></blockquote>"
        );
    }

    #[test]
    fn bsky_quote_block_html_uses_handle_without_display_name() {
        let html = bsky_quote_block_html(
            None,
            "alice.bsky.social",
            "https://bsky.app/profile/did:plc:abc",
            "hi",
        )
        .unwrap();
        assert!(
            html.starts_with("<blockquote><p><code>@\u{2060}alice.bsky.social</code><br>hi</p>")
        );
    }

    #[test]
    fn bsky_quote_block_html_turns_link_markers_into_anchors() {
        let html = bsky_quote_block_html(
            None,
            "a.bsky.social",
            "https://bsky.app/profile/x",
            "see [bsky.app/settings/pri...](https://bsky.app/settings/privacy) and [not a link] (x) [y](javascript:z) [a] b [c](https://c.example)",
        )
        .unwrap();
        assert!(html.contains(
            r#"see <a href="https://bsky.app/settings/privacy" rel="nofollow noopener">bsky.app/settings/pri...</a> and [not a link] (x) [y](javascript:z) [a] b <a href="https://c.example" rel="nofollow noopener">c</a>"#
        ));
    }

    #[test]
    fn bsky_quote_block_html_wraps_handle_mentions_in_code() {
        let html = bsky_quote_block_html(
            None,
            "a.bsky.social",
            "https://bsky.app/profile/x",
            "hi @bob.bsky.social. mail me@example.com or @carol (@dave.example.jp)",
        )
        .unwrap();
        assert!(html.contains(
            "hi <code>@\u{2060}bob.bsky.social</code>. mail me@example.com or @carol (<code>@\u{2060}dave.example.jp</code>)"
        ));
    }

    #[test]
    fn bsky_quote_block_html_skips_empty_body() {
        assert_eq!(
            bsky_quote_block_html(
                Some("Alice"),
                "alice.bsky.social",
                "https://bsky.app/profile/x",
                "  \n "
            ),
            None
        );
    }

    #[test]
    fn at_uri_to_bsky_profile_url_extracts_did() {
        assert_eq!(
            at_uri_to_bsky_profile_url("at://did:plc:abc/app.bsky.feed.post/3k"),
            Some("https://bsky.app/profile/did:plc:abc".to_owned())
        );
        assert_eq!(at_uri_to_bsky_profile_url("https://example.com"), None);
    }

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
