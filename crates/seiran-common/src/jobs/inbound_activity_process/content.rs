use super::*;


/// HTML エンティティのデコード（`strip_html` と `ap_content_to_markdown_body` で共有）。
fn decode_html_entities(s: &str) -> String {
    html_escape::decode_html_entities(s).into_owned()
}

/// プレーンテキストへの単純な HTML タグ除去（エンティティも簡易デコード）。
pub fn strip_html(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                result.push(' ');
            }
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    decode_html_entities(&result)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// HTML を「地の文」と「`<a href>`リンク」のセグメント列に分解する（`<a>` 以外のタグは
/// すべて空白除去、ネストしたタグ（`<span>` 等）はリンクの内側テキストからも除去する）。
/// 閉じタグの無い不正な HTML でも無限ループ・パニックせず、そこまでの内容で打ち切る。
enum HtmlSegment {
    Text(String),
    Link {
        href: String,
        text: String,
        /// `<a>` の `class` 属性に `mention`/`u-url` トークンが含まれるか。多くのFedi実装
        /// （Mastodon等）はメンションアンカーに microformats クラスを付与するが、そのhrefは
        /// 人間向けプロフィールURLで、`tag`配列のMention.hrefと一致しないことがある
        /// （後者はAPアクターURI）。class情報を残しておき、href不一致時のフォールバック
        /// 判定に使う。
        is_mention_class: bool,
        /// `<a>` の `rel` に `tag` トークン、または `class` に `hashtag` トークンが含まれるか。
        /// Mastodon等はハッシュタグアンカーにも `class="mention hashtag"` を付与する（`mention`
        /// トークンを共有する）ため、`is_mention_class` だけでは真のメンションと区別できない
        /// （実機確認せずとも仕様上判明: Mastodonのハッシュタグリンクは常に `rel="tag"` を持つ）。
        /// メンション解決より先にこちらを判定し、ハッシュタグなら通常のURLリンクとして扱う。
        is_hashtag: bool,
    },
}

/// 非アンカータグ1個が地の文にもたらす区切り文字を返す（改行系タグのみ `\n`/`\n\n`、
/// それ以外は半角スペース1個）。Mastodon等は改行を生の `\n` ではなく `<br>`/`<p>` で
/// 表現するため、単純にすべてスペースへ潰すと改行が失われてしまう。
fn tag_break_text(tag_inner: &str) -> &'static str {
    let trimmed = tag_inner.trim().trim_end_matches('/').trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "br" => "\n",
        "/p" | "/div" => "\n\n",
        _ => " ",
    }
}

fn tokenize_anchors(html: &str) -> Vec<HtmlSegment> {
    let chars: Vec<char> = html.chars().collect();
    let mut segments = Vec::new();
    let mut text_buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            text_buf.push(chars[i]);
            i += 1;
            continue;
        }

        // タグ全体（`<...>`）を読む。閉じる `>` が無ければ末尾までを1タグとみなす。
        let mut j = i + 1;
        while j < chars.len() && chars[j] != '>' {
            j += 1;
        }
        let tag_inner: String = chars[i + 1..j].iter().collect();
        let after_tag = if j < chars.len() { j + 1 } else { j };

        let trimmed = tag_inner.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        let is_anchor_open = (lower == "a" || lower.starts_with("a ") || lower.starts_with("a\t"))
            && !trimmed.ends_with('/');

        if !is_anchor_open {
            text_buf.push_str(tag_break_text(&tag_inner));
            i = after_tag;
            continue;
        }

        if !text_buf.is_empty() {
            segments.push(HtmlSegment::Text(std::mem::take(&mut text_buf)));
        }
        let href = extract_href_attr(&tag_inner);
        // Mastodon等はメンションアンカーに `class="u-url mention"` を付与するが、その href は
        // 人間向けプロフィールURLで `tag`配列のMention.href（APアクターURI）とは別物のことが
        // 多い。class情報を残し、href不一致時のフォールバック判定に使う（後述）。
        let is_mention_class = extract_class_tokens(&tag_inner)
            .iter()
            .any(|c| c == "mention" || c == "u-url");
        let is_hashtag = extract_class_tokens(&tag_inner)
            .iter()
            .any(|c| c == "hashtag")
            || extract_attr(&tag_inner, "rel")
                .map(|r| r.split_whitespace().any(|t| t.eq_ignore_ascii_case("tag")))
                .unwrap_or(false);
        i = after_tag;

        // `</a>` まで読み、ネストしたタグは除去してテキストだけ残す。
        let mut inner_text = String::new();
        let mut in_inner_tag = false;
        while i < chars.len() {
            if chars[i] == '<' {
                let ahead: String = chars[i + 1..]
                    .iter()
                    .take(2)
                    .collect::<String>()
                    .to_ascii_lowercase();
                if ahead == "/a" {
                    // `</a...>` という閉じタグ（属性・空白付きの `</a >` 等も含む）。'>' まで読み飛ばす。
                    let mut k = i + 1;
                    while k < chars.len() && chars[k] != '>' {
                        k += 1;
                    }
                    i = if k < chars.len() { k + 1 } else { k };
                    break;
                }
                in_inner_tag = true;
            }
            if chars[i] == '>' {
                in_inner_tag = false;
                i += 1;
                continue;
            }
            if !in_inner_tag {
                inner_text.push(chars[i]);
            }
            i += 1;
        }

        let inner_text = decode_html_entities(inner_text.trim());
        match href {
            Some(h) if !inner_text.is_empty() => {
                segments.push(HtmlSegment::Link {
                    href: h,
                    text: inner_text,
                    is_mention_class,
                    is_hashtag,
                });
            }
            _ => {
                if !inner_text.is_empty() {
                    segments.push(HtmlSegment::Text(inner_text));
                }
            }
        }
        text_buf.push(' ');
    }
    if !text_buf.is_empty() {
        segments.push(HtmlSegment::Text(text_buf));
    }
    segments
}

/// タグの中身（`a href="..." class="..."` のような属性文字列）から指定した属性の値を抽出する。
fn extract_attr(tag_inner: &str, attr_name: &str) -> Option<String> {
    let lower = tag_inner.to_ascii_lowercase();
    let attr_lower = attr_name.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(rel_idx) = lower[search_from..].find(&attr_lower) {
        let idx = search_from + rel_idx;
        // 属性名の直前が英数字だと別属性名の一部（例: "href" 検索時の "xhref"）なので誤検出を避ける。
        let boundary_ok = idx == 0 || !lower.as_bytes()[idx - 1].is_ascii_alphanumeric();
        let after = &tag_inner[idx + attr_name.len()..];
        let after_trimmed = after.trim_start();
        if boundary_ok && after_trimmed.starts_with('=') {
            let value_part = after_trimmed[1..].trim_start();
            if let Some(quote) = value_part.chars().next() {
                if quote == '"' || quote == '\'' {
                    let rest = &value_part[quote.len_utf8()..];
                    if let Some(end) = rest.find(quote) {
                        return Some(rest[..end].to_string());
                    }
                }
            }
        }
        search_from = idx + attr_name.len();
    }
    None
}

fn extract_href_attr(tag_inner: &str) -> Option<String> {
    extract_attr(tag_inner, "href")
}

/// `class` 属性値を空白区切りのトークン列として返す（無ければ空）。
fn extract_class_tokens(tag_inner: &str) -> Vec<String> {
    extract_attr(tag_inner, "class")
        .map(|c| {
            c.split_whitespace()
                .map(|s| s.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

/// URL からホスト名部分を取り出す（`https://host/path?q#f` → `host`）。
fn extract_host(url: &str) -> Option<&str> {
    let without_scheme = url.split("://").nth(1)?;
    let host = without_scheme.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then_some(host)
}

/// `tag.name` が `@user` のようにドメイン省略の場合、`tag.href` のホスト名を補って
/// `@user@host` の完全修飾形にする。**Misskeyは自己言及メンション（投稿者自身への `@user`）の
/// `name` をローカルドメイン省略で送ってくることがある**（実機確認: `attributedTo` と同一の
/// アクターへのメンションで `name: "@yuba"` のみ、`href` はアクターURIそのもの）。
fn qualify_mention_name(name: &str, href: &str) -> String {
    let username = name.trim_start_matches('@');
    if username.contains('@') {
        return name.to_string(); // 既に完全修飾
    }
    match extract_host(href) {
        Some(host) => format!("@{}@{}", username, host),
        None => name.to_string(),
    }
}

/// AP Note の Mention タグ（`tag`配列の `{"type":"Mention","href":"...","name":"@user@host"}`）
/// と `href` が一致する場合、その `name`（完全修飾済み）を返す。
fn find_mention_name_by_href(href: &str, tags: &[serde_json::Value]) -> Option<String> {
    tags.iter()
        .find(|t| t["type"].as_str() == Some("Mention") && t["href"].as_str() == Some(href))
        .and_then(|t| Some(qualify_mention_name(t["name"].as_str()?, href)))
}

/// `<a>` の内側テキスト（例: `@bob`）のユーザー名部分と `tag`配列内 Mention の `name` の
/// ユーザー名部分が一致するものを探す（`<a href>` が `tag[].href` と完全一致しない実装への
/// フォールバック）。**同名ユーザーが複数の Mention として存在する場合**（例: 投稿者自身への
/// `@yuba` と別インスタンスの `@yuba@fedibird.com` が同一Note内に共存するケース、実機確認）に
/// 誤った方へマッチしないよう、まず `<a href>` と `tag.href` のホスト名が一致するものを優先し、
/// 見つからなければユーザー名のみの一致にフォールバックする。
fn find_mention_name_by_inner_text(
    anchor_href: &str,
    inner_text: &str,
    tags: &[serde_json::Value],
) -> Option<String> {
    let inner_username = inner_text.trim_start_matches('@').split('@').next()?;
    if inner_username.is_empty() {
        return None;
    }
    let mentions: Vec<&serde_json::Value> = tags
        .iter()
        .filter(|t| t["type"].as_str() == Some("Mention"))
        .collect();

    let username_matches = |t: &&serde_json::Value| -> bool {
        t["name"]
            .as_str()
            .and_then(|name| name.trim_start_matches('@').split('@').next())
            .map(|name_username| name_username.eq_ignore_ascii_case(inner_username))
            .unwrap_or(false)
    };

    if let Some(anchor_host) = extract_host(anchor_href) {
        if let Some(found) = mentions.iter().find(|t| {
            username_matches(t)
                && t["href"]
                    .as_str()
                    .and_then(extract_host)
                    .map(|h| h.eq_ignore_ascii_case(anchor_host))
                    .unwrap_or(false)
        }) {
            let name = found["name"].as_str().unwrap_or_default();
            let href = found["href"].as_str().unwrap_or_default();
            return Some(qualify_mention_name(name, href));
        }
    }

    // ホスト一致が見つからない場合のみ、ユーザー名だけのフォールバック一致を使う。
    mentions.iter().find(|t| username_matches(t)).map(|t| {
        let name = t["name"].as_str().unwrap_or_default();
        let href = t["href"].as_str().unwrap_or_default();
        qualify_mention_name(name, href)
    })
}

/// AP Note のメンションアンカーが示す表示用メンション文字列（`@user@host`）を解決する。
///
/// 1. `href` が `tag`配列の Mention.href と完全一致 → その `name`（完全修飾済み）を使う
/// 2. `<a>` の `class` に `mention`/`u-url` があり、`href` は不一致だが `tag`配列の中に
///    （ホスト名優先で）ユーザー名が一致する Mention がある（Mastodon等は `<a>` の href に
///    人間向けプロフィールURL、`tag[].href` にAPアクターURIを使い分けるため、両者が食い違う
///    ことがある）→ その `name`
/// 3. 上記いずれにも該当しないが `class` から見てメンションらしい → `<a>` の内側テキストを
///    使う。ドメイン部分が省略されている（`@bob` のように単一`@`のみ）場合は、投稿元アクターの
///    ドメイン（`sender_domain`）を補って `@bob@sender_domain` の完全修飾形にする
///    （投稿元インスタンス内の相対メンション表記への対応）。
///
/// メンションと判断できなければ `None`（呼び出し側は通常のURLリンクとして扱う）。
///
/// `is_hashtag` が真の場合は上記いずれも試みず即座に `None` を返す。Mastodon等は
/// ハッシュタグアンカーにも `class="mention hashtag"` を付与する（`mention` トークンを
/// メンションと共有する）ため、`is_mention_class` だけで判定すると `#foo` が
/// `@#foo@sender_domain` のような壊れたメンション文字列に誤変換されてしまう。
fn resolve_ap_mention_text(
    href: &str,
    inner_text: &str,
    is_mention_class: bool,
    is_hashtag: bool,
    tags: &[serde_json::Value],
    sender_domain: &str,
) -> Option<String> {
    if is_hashtag {
        return None;
    }
    if let Some(name) = find_mention_name_by_href(href, tags) {
        return Some(name);
    }
    if !is_mention_class {
        return None;
    }
    if let Some(name) = find_mention_name_by_inner_text(href, inner_text, tags) {
        return Some(name);
    }
    // tag配列に対応エントリが無くても class から見てメンションらしいので、内側テキストを
    // 完全修飾メンションへ正規化して採用する（本拠地サーバーへの直リンクを避けるため）。
    let username = inner_text.trim_start_matches('@');
    if username.is_empty() {
        return None;
    }
    Some(if username.contains('@') || sender_domain.is_empty() {
        format!("@{}", username)
    } else {
        format!("@{}@{}", username, sender_domain)
    })
}

/// 改行（`\n`）を保持したまま、行内の連続空白だけを1個にまとめる。3個以上連続する改行は
/// 2個（＝空行1つ）に、前後の空行はtrimする。`<br>`/`</p>`由来の改行と、タグ跡の半角スペースが
/// 混在した文字列を、Misskey本家の `note.text` のような自然な改行付きプレーンテキストにする。
fn normalize_whitespace_preserving_newlines(s: &str) -> String {
    let joined = s
        .split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");

    let mut result = String::with_capacity(joined.len());
    let mut newline_run = 0usize;
    for c in joined.chars() {
        if c == '\n' {
            newline_run += 1;
        } else {
            if newline_run > 0 {
                result.push_str(&"\n".repeat(newline_run.min(2)));
                newline_run = 0;
            }
            result.push(c);
        }
    }
    result.trim_matches('\n').to_string()
}

/// AP Note の `content`（HTML）を、内部リンクマーカー `[表示テキスト](URL)`（Markdown
/// リンク記法）を埋め込んだプレーンテキストへ変換する。`strip_html` との違いは `<a href>`
/// をリンクとして保持する点と、`<br>`/`</p>` を改行として保持する点。ただしメンションと
/// 判定されたリンクはMarkdownリンクで包まず、`@user@host` というプレーンテキストに正規化する
/// （メンションはフロント側のメンション検出に委ねる。判定方法は `resolve_ap_mention_text`
/// 参照）。一般の URL リンク・ハッシュタグのアンカーはそのまま `[text](url)` に変換する。
///
/// `sender_domain` はこのNoteの投稿者（アクター）のドメイン。`class="mention"` はあるが
/// `tag`配列に対応エントリが無くドメイン省略のメンション（`@bob`）しか得られない場合、
/// このドメインを補って完全修飾形（`@bob@sender_domain`）にする。
pub fn ap_content_to_markdown_body(
    content_html: &str,
    tags: &[serde_json::Value],
    sender_domain: &str,
) -> String {
    let mut out = String::new();
    for seg in tokenize_anchors(content_html) {
        match seg {
            HtmlSegment::Text(t) => out.push_str(&t),
            HtmlSegment::Link {
                href,
                text,
                is_mention_class,
                is_hashtag,
            } => {
                if let Some(name) = resolve_ap_mention_text(
                    &href,
                    &text,
                    is_mention_class,
                    is_hashtag,
                    tags,
                    sender_domain,
                ) {
                    out.push_str(&name);
                } else {
                    out.push('[');
                    out.push_str(&text);
                    out.push_str("](");
                    out.push_str(&href);
                    out.push(')');
                }
            }
        }
    }
    normalize_whitespace_preserving_newlines(&decode_html_entities(&out))
}

/// メンション/ハッシュタグの `<a>` の `href` だけを内部パス（`/@user@host`・`/tags/xxx`）へ
/// 書き換え、それ以外のHTML構造（ネストしたタグ・属性・非アンカー要素・地の文）は一切変更せず
/// バイト単位でそのまま残す。判定ロジックは `resolve_ap_mention_text` 系を`ap_content_to_markdown_body`
/// と全く同じ精度で再利用する（`href`完全一致優先→class由来のフォールバック→内側テキストの
/// 完全修飾化）。`sanitize_ap_content_html` の前処理として使う。
///
/// `ap_content_to_markdown_body`の`tokenize_anchors`とは別実装（あちらは非アンカータグを
/// 空白/改行1個に潰してしまうため、構造保持が目的のここでは使えない）。
fn rewrite_mention_hashtag_hrefs(
    html: &str,
    tags: &[serde_json::Value],
    sender_domain: &str,
) -> String {
    let chars: Vec<char> = html.chars().collect();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            out.push(chars[i]);
            i += 1;
            continue;
        }

        let mut j = i + 1;
        while j < chars.len() && chars[j] != '>' {
            j += 1;
        }
        let tag_inner: String = chars[i + 1..j].iter().collect();
        let after_tag = if j < chars.len() { j + 1 } else { j };

        let trimmed = tag_inner.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        let is_anchor_open = (lower == "a" || lower.starts_with("a ") || lower.starts_with("a\t"))
            && !trimmed.ends_with('/');

        if !is_anchor_open {
            out.extend(&chars[i..after_tag]);
            i = after_tag;
            continue;
        }

        let href = extract_href_attr(&tag_inner).unwrap_or_default();
        let is_mention_class = extract_class_tokens(&tag_inner)
            .iter()
            .any(|c| c == "mention" || c == "u-url");
        let is_hashtag = extract_class_tokens(&tag_inner)
            .iter()
            .any(|c| c == "hashtag")
            || extract_attr(&tag_inner, "rel")
                .map(|r| r.split_whitespace().any(|t| t.eq_ignore_ascii_case("tag")))
                .unwrap_or(false);
        i = after_tag;

        let inner_start = i;
        let mut plain_text = String::new();
        let mut in_inner_tag = false;
        let mut closed = false;
        while i < chars.len() {
            if chars[i] == '<' {
                let ahead: String = chars[i + 1..]
                    .iter()
                    .take(2)
                    .collect::<String>()
                    .to_ascii_lowercase();
                if ahead == "/a" {
                    let mut k = i + 1;
                    while k < chars.len() && chars[k] != '>' {
                        k += 1;
                    }
                    let inner_end = i;
                    let raw_inner: String = chars[inner_start..inner_end].iter().collect();
                    i = if k < chars.len() { k + 1 } else { k };

                    let decoded_text = decode_html_entities(plain_text.trim());
                    let new_href = if is_hashtag {
                        let tag_text = decoded_text.trim_start_matches('#');
                        (!tag_text.is_empty()).then(|| {
                            format!("/tags/{}", urlencoding::encode(&tag_text.to_lowercase()))
                        })
                    } else {
                        resolve_ap_mention_text(
                            &href,
                            &decoded_text,
                            is_mention_class,
                            is_hashtag,
                            tags,
                            sender_domain,
                        )
                        .map(|name| format!("/@{}", name.trim_start_matches('@')))
                    };

                    out.push_str("<a href=\"");
                    match &new_href {
                        Some(internal) => out.push_str(&escape_html_attr(internal)),
                        None => out.push_str(&href),
                    }
                    out.push_str("\">");
                    out.push_str(&raw_inner);
                    out.push_str("</a>");
                    closed = true;
                    break;
                }
                in_inner_tag = true;
            }
            if chars[i] == '>' {
                in_inner_tag = false;
                i += 1;
                continue;
            }
            if !in_inner_tag {
                plain_text.push(chars[i]);
            }
            i += 1;
        }
        if !closed {
            // 閉じタグ `</a>` が無い不正なHTML。ここまでの内容をそのまま出力して打ち切る
            // （`tokenize_anchors`と同じ「パニックしない」方針）。
            out.push_str("<a href=\"");
            out.push_str(&href);
            out.push_str("\">");
            out.extend(&chars[inner_start..i]);
        }
    }
    out
}

/// HTML属性値として安全な形にエスケープする（`&`/`"`/`<`/`>`）。ここでは新規生成した内部パス
/// （`/@user@host`・`/tags/xxx`）にのみ使う。元のHTMLから抽出した`href`はソース側で既に
/// エスケープ済みの生文字列なので、そのまま書き戻す（二重エスケープを避けるため通さない）。
fn escape_html_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `style` 属性値が `text-align: left|right|center|justify` という1プロパティのみで
/// 構成されているか判定する。それ以外のCSSプロパティ・`!important`・複数プロパティの
/// 混入は許可しない（CSSインジェクション面を最小化する）。
fn is_allowed_style_value(value: &str) -> bool {
    let v = value.trim().trim_end_matches(';').trim();
    let Some(rest) = v.strip_prefix("text-align") else {
        return false;
    };
    let rest = rest.trim_start();
    let Some(rest) = rest.strip_prefix(':') else {
        return false;
    };
    matches!(rest.trim(), "left" | "right" | "center" | "justify")
}

/// Misskey/Fedibirdが引用時に自動付加する`RE:`/`QT:`フォールバック行を、HTML本文
/// （`content_html`）の末尾から取り除く。`strip_quote_fallback_line`のHTML版
/// （プレーンテキストの`\n`区切りの代わりに`<br>`をおおよその行区切りとして使う）。
/// `<br>`が無い（フォールバック行しかない）場合は空文字列を返す。
pub(super) fn strip_quote_fallback_line_html(html: &str, quote_uri: &str) -> String {
    fn strip_tags(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut in_tag = false;
        for c in s.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(c),
                _ => {}
            }
        }
        decode_html_entities(out.trim())
    }

    let trimmed = html.trim_end();
    let last_br = {
        let lower = trimmed.to_ascii_lowercase();
        lower.rfind("<br")
    };
    let (before, after) = match last_br {
        Some(idx) => {
            // `<br>`/`<br/>`/`<br />` いずれの終端 `>` も飛ばす。
            let close = trimmed[idx..].find('>').map(|o| idx + o + 1).unwrap_or(idx);
            (&trimmed[..idx], &trimmed[close..])
        }
        None => ("", trimmed),
    };

    let last_line = strip_tags(after);
    let is_fallback = (last_line.starts_with("RE:") || last_line.starts_with("QT:"))
        && last_line.contains(quote_uri);

    if is_fallback {
        before.trim_end().to_string()
    } else {
        html.to_string()
    }
}

/// AP Note の `content`（HTML）を、意味的な構造（引用・強調・ルビ・リンク等）を保持したまま
/// サニタイズする。`ap_content_to_markdown_body`（プレーンテキスト化・`body`列用）とは別に、
/// `content_html`列（seiran Web UIでのリッチ表示専用、リモートFedi投稿のみ）を作るために使う。
///
/// 1. `rewrite_mention_hashtag_hrefs` でメンション/ハッシュタグの`<a>`だけ内部リンクへ書き換え。
/// 2. allowlist（タグ・属性）でサニタイズ（`ammonia`）。`class`はどのタグからも除去し、
///    `style`は`text-align`のみ許可、`href`/`src`は`http`/`https`スキームのみ許可する。
///    `rel`/`target`はここでは一切保持しない（信用できるのはこちらが強制する値だけであるべき
///    なので、フロントのレンダラ側で固定値を付与する）。
pub fn sanitize_ap_content_html(
    content_html: &str,
    tags: &[serde_json::Value],
    sender_domain: &str,
) -> String {
    let rewritten = rewrite_mention_hashtag_hrefs(content_html, tags, sender_domain);

    let allowed_tags: HashSet<&str> = [
        "br",
        "p",
        "div",
        "a",
        "b",
        "i",
        "s",
        "code",
        "pre",
        "blockquote",
        "ruby",
        "rt",
        "rp",
        "h1",
        "h2",
        "figure",
        "img",
        "ul",
        "ol",
        "li",
        "small",
        "center",
    ]
    .into_iter()
    .collect();

    let mut tag_attributes: std::collections::HashMap<&str, HashSet<&str>> =
        std::collections::HashMap::new();
    tag_attributes.insert("a", ["href"].into_iter().collect());
    tag_attributes.insert(
        "img",
        ["src", "alt", "width", "height"].into_iter().collect(),
    );

    ammonia::Builder::new()
        .tags(allowed_tags)
        .tag_attributes(tag_attributes)
        .generic_attributes(["style"].into_iter().collect())
        .url_schemes(["http", "https"].into_iter().collect())
        // `rel`/`target`はここでは一切保持しない（フロントのレンダラ側で固定値を強制する）。
        // ammoniaのデフォルトは`<a>`に`rel="noopener noreferrer"`を自動付与するため明示的に無効化する。
        .link_rel(None)
        .attribute_filter(|_element, attribute, value| {
            if attribute == "style" {
                if is_allowed_style_value(value) {
                    Some(value.trim().to_string().into())
                } else {
                    None
                }
            } else {
                Some(value.into())
            }
        })
        .clean(&rewritten)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ap_content_to_markdown_body_converts_plain_link() {
        let html = r#"<p>見て <a href="https://example.com/foo">example.com/foo</a> だよ</p>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "見て [example.com/foo](https://example.com/foo) だよ");
    }

    #[test]
    fn ap_content_to_markdown_body_mention_becomes_plain_handle_text() {
        // メンションは Markdown リンクで包まず、tag.name（フルのメンション文字列）を
        // そのままのテキストにする。フロントの MFM 描画コンポーネントが `@user@host`
        // パターンを検出してプロフィールリンクへ変換する前提。
        let html = r#"<p><a href="https://example.social/users/alice" class="u-url mention">@<span>alice</span></a> こんにちは</p>"#;
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://example.social/users/alice",
            "name": "@alice@example.social"
        })];
        let body = ap_content_to_markdown_body(html, &tags, "example.social");
        assert_eq!(body, "@alice@example.social こんにちは");
    }

    #[test]
    fn ap_content_to_markdown_body_mention_class_with_mismatched_href_falls_back_to_tag_username_match(
    ) {
        // Mastodon等は <a href> に人間向けプロフィールURL、tag[].href にAPアクターURIを使う
        // ため両者が食い違うことがある。href完全一致に失敗しても、tag配列の中からユーザー名が
        // 一致する Mention を見つけて name を採用し、本拠地サーバーへの直リンクにはしない。
        let html =
            r#"<p><a href="https://example.social/@bob" class="u-url mention">@bob</a> hi</p>"#;
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://example.social/users/bob",
            "name": "@bob@example.social"
        })];
        let body = ap_content_to_markdown_body(html, &tags, "example.social");
        assert_eq!(body, "@bob@example.social hi");
    }

    #[test]
    fn ap_content_to_markdown_body_mention_class_without_tag_entry_gets_sender_domain_appended() {
        // tag配列に対応エントリが全く無い場合でも、class=mention なら本拠地サーバーへの
        // 直リンクにはせず、投稿元アクターのドメイン（sender_domain）を補って完全修飾形にする。
        let html =
            r#"<p><a href="https://example.social/@carol" class="u-url mention">@carol</a> yo</p>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "@carol@example.social yo");
    }

    #[test]
    fn ap_content_to_markdown_body_self_mention_with_domain_omitted_name_gets_qualified() {
        // 実機確認（reax.work, Misskey系）: 投稿者自身への自己言及メンションは
        // tag.name がローカルドメイン省略の "@yuba" になることがある。href
        // （アクターURI）からホスト名を補って完全修飾形にする。
        let html = r#"<a href="https://reax.work/@yuba" class="u-url mention">@yuba</a>"#;
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://reax.work/users/9dohp6knpn",
            "name": "@yuba"
        })];
        let body = ap_content_to_markdown_body(html, &tags, "reax.work");
        assert_eq!(body, "@yuba@reax.work");
    }

    #[test]
    fn ap_content_to_markdown_body_same_username_different_hosts_do_not_cross_match() {
        // 実機確認: 同一Note内に同名ユーザー（投稿者自身 @yuba とは別インスタンスの
        // @yuba@fedibird.com）への2つのメンションがあると、ユーザー名だけでの一致判定では
        // 常に最初に見つかった方に誤マッチしてしまう。<a href> と tag.href のホスト名を
        // 突き合わせることで、それぞれ正しい tag に解決されなければならない。
        let html = concat!(
            r#"<a href="https://reax.work/@yuba" class="u-url mention">@yuba</a>"#,
            "<br />",
            r#"<a href="https://fedibird.com/@yuba" class="u-url mention">@yuba@fedibird.com</a>"#,
        );
        let tags = vec![
            serde_json::json!({
                "type": "Mention",
                "href": "https://reax.work/users/9dohp6knpn",
                "name": "@yuba"
            }),
            serde_json::json!({
                "type": "Mention",
                "href": "https://fedibird.com/users/yuba",
                "name": "@yuba@fedibird.com"
            }),
        ];
        let body = ap_content_to_markdown_body(html, &tags, "reax.work");
        assert_eq!(body, "@yuba@reax.work\n@yuba@fedibird.com");
    }

    #[test]
    fn ap_content_to_markdown_body_non_mention_link_with_mismatched_tags_stays_a_link() {
        // class に mention/u-url が無ければ通常のリンクとして扱う（本拠地サーバーへの
        // リンクになるのは意図通り、これは普通のURLリンクのケース）。
        let html = r#"<a href="https://example.com/article">記事</a>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "[記事](https://example.com/article)");
    }

    #[test]
    fn ap_content_to_markdown_body_hashtag_anchor_becomes_link_to_remote_tag_page() {
        let html = r#"<a href="https://example.social/tags/foo" rel="tag">#foo</a>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "[#foo](https://example.social/tags/foo)");
    }

    #[test]
    fn ap_content_to_markdown_body_real_mastodon_hashtag_anchor_with_mention_class_not_misparsed() {
        // 実際のMastodonはハッシュタグアンカーにも class="mention hashtag" を付与する
        // （メンションと `mention` トークンを共有する）。`rel="tag"` を見て先に弾かないと、
        // メンション解決ロジックに巻き込まれ `@#foo@example.social` のような壊れた
        // 文字列になってしまう（本テストが無い間に発生していた回帰）。
        let html = r#"<a href="https://example.social/tags/foo" class="mention hashtag" rel="tag">#foo</a>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "[#foo](https://example.social/tags/foo)");
    }

    #[test]
    fn ap_content_to_markdown_body_unclosed_anchor_does_not_panic() {
        let html = r#"text <a href="https://example.com">no closing tag"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        // 閉じタグが無くてもパニックせず、末尾までがリンクテキストとして扱われる。
        assert_eq!(body, "text [no closing tag](https://example.com)");
    }

    #[test]
    fn ap_content_to_markdown_body_preserves_markdown_like_plain_text() {
        // 元々 content 中に Markdown 風の文字列 `[text](url)` が含まれていた場合、
        // <a> タグ由来でなくてもそのまま通過する（フロント側のパーサーが解釈する）。
        let html = r#"<p>参考: [seiran](https://example.com/seiran)</p>"#;
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "参考: [seiran](https://example.com/seiran)");
    }

    #[test]
    fn ap_content_to_markdown_body_preserves_paragraph_and_br_newlines() {
        let html = "<p>1行目です</p><p>2行目<br>3行目です</p>";
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "1行目です\n\n2行目\n3行目です");
    }

    #[test]
    fn ap_content_to_markdown_body_collapses_excessive_blank_lines() {
        let html = "<p>foo</p><p></p><p></p><p>bar</p>";
        let body = ap_content_to_markdown_body(html, &[], "example.social");
        assert_eq!(body, "foo\n\nbar");
    }

    #[test]
    fn sanitize_ap_content_html_preserves_blockquote() {
        // 元不具合の直接的な回帰テスト（#233）: MFM引用構文由来の<blockquote>が
        // ap_content_to_markdown_bodyでは失われるが、sanitize_ap_content_htmlでは保持される。
        // `<blockquote>`はブロック要素なので、HTML5パーサーが`<p>`を自動的に閉じる
        // （実際のMisskey content HTMLもこの入れ子で届く。空`<p></p>`は無害）。
        let html = "<p><blockquote><span>quoted text</span></blockquote>after</p>";
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(
            out,
            "<p></p><blockquote>quoted text</blockquote>after<p></p>"
        );
    }

    #[test]
    fn sanitize_ap_content_html_preserves_ruby() {
        let html = "<ruby>漢字<rp>(</rp><rt>かんじ</rt><rp>)</rp></ruby>";
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, html);
    }

    #[test]
    fn sanitize_ap_content_html_preserves_inline_formatting() {
        let html = "<b>bold</b><i>italic</i><s>strike</s><code>code</code><pre>pre</pre>";
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, html);
    }

    #[test]
    fn sanitize_ap_content_html_rewrites_mention_href() {
        let html = r#"<a href="https://remote.example/@bob" class="u-url mention">@bob@remote.example</a>"#;
        let tags = vec![serde_json::json!({
            "type": "Mention",
            "href": "https://remote.example/@bob",
            "name": "@bob@remote.example"
        })];
        let out = sanitize_ap_content_html(html, &tags, "remote.example");
        assert_eq!(
            out,
            r#"<a href="/@bob@remote.example">@bob@remote.example</a>"#
        );
    }

    #[test]
    fn sanitize_ap_content_html_rewrites_hashtag_href() {
        let html = r#"<a href="https://remote.example/tags/foo" rel="tag">#foo</a>"#;
        let out = sanitize_ap_content_html(html, &[], "remote.example");
        assert_eq!(out, r#"<a href="/tags/foo">#foo</a>"#);
    }

    #[test]
    fn sanitize_ap_content_html_keeps_ordinary_link_href_but_drops_rel_target() {
        let html =
            r#"<a href="https://example.com/" rel="nofollow noopener" target="_blank">link</a>"#;
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, r#"<a href="https://example.com/">link</a>"#);
    }

    #[test]
    fn sanitize_ap_content_html_strips_disallowed_tag_and_script() {
        let html = "<script>alert(1)</script><span>plain</span>";
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, "plain");
    }

    #[test]
    fn sanitize_ap_content_html_rejects_javascript_scheme() {
        let html = r#"<a href="javascript:alert(1)">click</a>"#;
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert!(!out.contains("javascript:"), "got: {out}");
    }

    #[test]
    fn sanitize_ap_content_html_strips_class_attribute() {
        let html = r#"<p class="foo">text</p>"#;
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(out, "<p>text</p>");
    }

    #[test]
    fn sanitize_ap_content_html_keeps_only_text_align_style() {
        let html = r#"<div style="text-align: center">c</div><div style="color: red">r</div>"#;
        let out = sanitize_ap_content_html(html, &[], "example.social");
        assert_eq!(
            out,
            r#"<div style="text-align: center">c</div><div>r</div>"#
        );
    }

    #[test]
    fn strip_quote_fallback_line_html_removes_trailing_re_line() {
        let html = "<p>本文<br>RE: <a href=\"https://q.example/1\">https://q.example/1</a></p>";
        let out = strip_quote_fallback_line_html(html, "https://q.example/1");
        assert_eq!(out, "<p>本文");
    }

    #[test]
    fn strip_quote_fallback_line_html_keeps_unrelated_content() {
        let html = "<p>本文<br>RE: not a match</p>";
        let out = strip_quote_fallback_line_html(html, "https://q.example/1");
        assert_eq!(out, html);
    }

    #[test]
    fn test_strip_html_simple() {
        assert_eq!(strip_html("<p>Hello, world!</p>"), "Hello, world!");
        assert_eq!(
            strip_html("<b>bold</b> and <i>italic</i>"),
            "bold and italic"
        );
    }

    #[test]
    fn test_strip_html_entities() {
        assert_eq!(strip_html("<p>a &amp; b</p>"), "a & b");
        assert_eq!(strip_html("&lt;script&gt;"), "<script>");
        assert_eq!(strip_html("&quot;quoted&quot;"), "\"quoted\"");
        assert_eq!(strip_html("it&#39;s"), "it's");
        assert_eq!(strip_html("VisualArt&#039;s"), "VisualArt's");
        assert_eq!(strip_html("VisualArt&#x27;s"), "VisualArt's");
        assert_eq!(strip_html("VisualArt&apos;s"), "VisualArt's");
        assert_eq!(strip_html("a&nbsp;b"), "a b");
    }

    #[test]
    fn test_strip_html_empty() {
        assert_eq!(strip_html(""), "");
        assert_eq!(strip_html("   "), "");
        assert_eq!(strip_html("<br/>"), "");
    }
}
