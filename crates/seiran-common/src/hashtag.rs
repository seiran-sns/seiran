//! 投稿本文からのハッシュタグ抽出。
//!
//! ローカル投稿・AP受信・Bsky受信のいずれも、最終的な `posts.body` テキストに
//! `#foo` という文字列がリテラルに含まれる点は共通（AP由来のハッシュタグアンカーは
//! `[#foo](リモートのタグページURL)` という Markdown リンクに変換されるが、リンク
//! テキスト部分に `#foo` はそのまま残る）。そのため、プロトコル別の特別処理を持たず
//! 「最終的な body テキストを1回スキャンする」だけで3ソース共通のハッシュタグ抽出が
//! 成立する。

/// `text` からハッシュタグを抽出する（正規化済み・重複除去・出現順）。
///
/// - `#` の直前が半角英数字・アンダースコア・`/` の場合はマッチしない
///   （`https://x.com/a#b` のような URL フラグメント断片の誤検出を防ぐ）。ASCII限定で
///   チェックするのは、Unicode版 `is_alphanumeric()` だと日本語等の文字も真になり、
///   「今日も#猫」のように直前にスペースを置かない日本語の実利用パターンで直前文字が
///   誤って「英数字直後」と判定されタグとして検出できなくなるため（`mention.rs` の
///   `@` メンション走査と同じ理由・同じ対処）。
/// - タグ本体は Unicode 英数字（`char::is_alphanumeric()`）とアンダースコアのみ。
/// - タグ本体にアルファベットを1文字も含まないもの（`#2026`, `#000` 等の純数字列）は
///   除外する（Mastodon 等の慣行に合わせる。色コードや年号だけのハッシュタグ誤爆を防ぐ）。
/// - 正規化は `to_lowercase()`（グルーピング用の内部表現。画面表示は各投稿の body 原文の
///   大文字小文字をそのまま使う）。
pub fn extract_hashtags(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '#' {
            let prev_ok = i == 0
                || !(chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == '/');
            if prev_ok {
                let start = i + 1;
                let mut j = start;
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if j > start {
                    let raw: String = chars[start..j].iter().collect();
                    if raw.chars().any(|c| c.is_alphabetic()) {
                        let normalized = raw.to_lowercase();
                        if seen.insert(normalized.clone()) {
                            result.push(normalized);
                        }
                    }
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_ascii_hashtag() {
        assert_eq!(extract_hashtags("hello #world today"), vec!["world"]);
    }

    #[test]
    fn extracts_japanese_hashtag() {
        assert_eq!(extract_hashtags("今日も#猫 かわいい"), vec!["猫"]);
    }

    #[test]
    fn extracts_alphanumeric_mixed_hashtag() {
        assert_eq!(extract_hashtags("#vtuber2026 最高"), vec!["vtuber2026"]);
    }

    #[test]
    fn normalizes_case_for_grouping() {
        assert_eq!(extract_hashtags("#Foo と #foo"), vec!["foo"]);
    }

    #[test]
    fn extracts_from_markdown_link_text() {
        // AP由来のハッシュタグアンカーは `[#foo](https://remote/tags/foo)` という
        // Markdownリンクに変換されているが、リンクテキスト部分から抽出できる。
        assert_eq!(
            extract_hashtags("見て [#foo](https://example.social/tags/foo) です"),
            vec!["foo"]
        );
    }

    #[test]
    fn ignores_url_fragment() {
        assert_eq!(extract_hashtags("参照: https://x.com/a#section"), Vec::<String>::new());
    }

    #[test]
    fn ignores_pure_numeric_hashtag() {
        assert_eq!(extract_hashtags("#2026 年もよろしく #000"), Vec::<String>::new());
    }

    #[test]
    fn dedupes_and_preserves_first_occurrence_order() {
        assert_eq!(extract_hashtags("#foo #bar #foo"), vec!["foo", "bar"]);
    }

    #[test]
    fn no_hashtag_returns_empty() {
        assert_eq!(extract_hashtags("普通の文章です"), Vec::<String>::new());
    }
}
