//! notes ハンドラの入力検証（本文長・添付件数・リアクション内容）と HTML ユーティリティ。

use unicode_segmentation::UnicodeSegmentation;

use crate::error::ApiError;

/// Bsky 配信時の本文上限（書記素クラスタ数）。`/api/meta` の `maxNoteTextLength` にも使う。
pub const BSKY_MAX_TEXT_GRAPHEMES: usize = 300;
/// Bsky 配信時の本文上限（バイト数）。
const BSKY_MAX_TEXT_BYTES: usize = 3_000;
/// Fedi のみ配信時の本文上限（書記素クラスタ数）。
const FEDI_MAX_TEXT_GRAPHEMES: usize = 3_000;
/// Fedi のみ配信時の本文上限（バイト数）。
const FEDI_MAX_TEXT_BYTES: usize = 10_000;
/// DM の宛先に Bsky アクターが含まれる場合の本文上限（`chat.bsky.convo` の実仕様値、
/// 書記素クラスタ数）。
pub const BSKY_DM_MAX_TEXT_GRAPHEMES: usize = 1_000;
/// 同上、バイト数。
const BSKY_DM_MAX_TEXT_BYTES: usize = 10_000;

/// 投稿文字数を配信先に応じたバイト数・書記素クラスタ数の上限で検証する。
///
/// `bsky_text` は Bsky 配信する場合に渡す、メンション変換後（`convert_mentions_for_bsky`
/// 適用後）のテキスト。`@user` → `@user.example.com` のような展開でバイト数・書記素数が
/// 増えうるため、Bsky の厳密な上限（300 書記素・3000 バイト）は元の入力テキストではなく
/// この変換後テキストに対してチェックする（呼び出し元が投稿受理前に同期的に変換すること）。
/// `None`（Bsky 非配信）の場合は元の `text` を Fedi 向けの緩い上限でチェックする。
pub fn validate_text_length(text: &str, bsky_text: Option<&str>) -> Result<(), ApiError> {
    let (checked, max_bytes, max_graphemes): (&str, usize, usize) = match bsky_text {
        Some(bt) => (bt, BSKY_MAX_TEXT_BYTES, BSKY_MAX_TEXT_GRAPHEMES),
        None => (text, FEDI_MAX_TEXT_BYTES, FEDI_MAX_TEXT_GRAPHEMES),
    };
    if checked.len() > max_bytes || checked.graphemes(true).count() > max_graphemes {
        return Err(ApiError::BadRequest("TEXT_TOO_LONG".to_owned()));
    }
    Ok(())
}

/// DM本文の文字数を検証する。宛先にBskyアクターが含まれる場合は `chat.bsky.convo` の
/// 実仕様上限（1000書記素・10000バイト）、含まれなければ通常のFedi向け上限を使う。
pub fn validate_dm_text_length(text: &str, has_bsky_recipient: bool) -> Result<(), ApiError> {
    let (max_bytes, max_graphemes) = if has_bsky_recipient {
        (BSKY_DM_MAX_TEXT_BYTES, BSKY_DM_MAX_TEXT_GRAPHEMES)
    } else {
        (FEDI_MAX_TEXT_BYTES, FEDI_MAX_TEXT_GRAPHEMES)
    };
    if text.len() > max_bytes || text.graphemes(true).count() > max_graphemes {
        return Err(ApiError::BadRequest("TEXT_TOO_LONG".to_owned()));
    }
    Ok(())
}

/// 添付ファイル ID の件数・形式を検証する（件数上限 10、i64 としてパース可能か）。
pub fn validate_attachment_ids(ids: &[String]) -> Result<(), ApiError> {
    if ids.len() > 10 {
        return Err(ApiError::BadRequest("添付ファイルは最大10件です".to_owned()));
    }
    if ids.iter().any(|s| s.parse::<i64>().is_err()) {
        return Err(ApiError::BadRequest("INVALID_ATTACHMENT_ID".to_owned()));
    }
    Ok(())
}

/// リアクション内容の書記素クラスタ数の安全上限（`emojis::get` の完全一致チェックの前段で
/// 極端に長い文字列を弾くためのもの。実際の絵文字判定はこの定数ではなく下記の完全一致で行う）。
const MAX_REACTION_CONTENT_LEN: usize = 32;

/// リアクション内容を検証し、trim 済みの文字列を返す。
///
/// 「絵文字リアクション」という以上、Unicode 絵文字（単体・肌色/性別修飾・ZWJ結合・国旗・
/// キーキャップ等の RGI シーケンスを含む）以外の文字列は許可しない。`:shortcode:` のような
/// カスタム絵文字ショートコードも現状未対応のため拒否する。判定は `emojis` crate
/// （Unicode 公式の emoji-test.txt 準拠データ）による完全一致で行う。
pub fn validate_reaction_content(raw: &str) -> Result<String, ApiError> {
    let content = raw.trim().to_string();
    if content.is_empty() || content.graphemes(true).count() > MAX_REACTION_CONTENT_LEN {
        return Err(ApiError::BadRequest("INVALID_REACTION_CONTENT".to_owned()));
    }
    if emojis::get(&content).is_none() {
        return Err(ApiError::BadRequest("INVALID_REACTION_CONTENT".to_owned()));
    }
    Ok(content)
}

/// HTML タグを取り除き、基本エンティティを復元する。
pub fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::{strip_html_tags, validate_reaction_content, validate_text_length};

    #[test]
    fn validate_text_length_bsky_checks_converted_text_not_raw() {
        // 変換後（bsky_text）が上限を超えていれば、元の text が短くても弾く
        let raw = "@a";
        let converted = "a".repeat(super::BSKY_MAX_TEXT_GRAPHEMES + 1);
        assert!(validate_text_length(raw, Some(&converted)).is_err());
    }

    #[test]
    fn validate_text_length_bsky_within_limit_ok() {
        let raw = "hello";
        let converted = "hello.seiran.example";
        assert!(validate_text_length(raw, Some(converted)).is_ok());
    }

    #[test]
    fn validate_text_length_no_bsky_uses_raw_text_with_looser_limit() {
        // Bsky 上限を超えるが Fedi の緩い上限には収まる長さ
        let text = "a".repeat(super::BSKY_MAX_TEXT_GRAPHEMES + 100);
        assert!(validate_text_length(&text, None).is_ok());
    }

    #[test]
    fn validate_text_length_no_bsky_still_enforces_fedi_limit() {
        let text = "a".repeat(super::FEDI_MAX_TEXT_GRAPHEMES + 1);
        assert!(validate_text_length(&text, None).is_err());
    }

    #[test]
    fn strip_html_tags_removes_tags_and_decodes_entities() {
        assert_eq!(strip_html_tags("<p>a &amp; b</p>"), "a & b");
        assert_eq!(strip_html_tags("&lt;script&gt;"), "<script>");
    }

    #[test]
    fn strip_html_tags_empty() {
        assert_eq!(strip_html_tags(""), "");
        assert_eq!(strip_html_tags("<br/>"), "");
    }

    #[test]
    fn validate_reaction_content_accepts_basic_emoji() {
        assert_eq!(validate_reaction_content("🎉").unwrap(), "🎉");
        assert_eq!(validate_reaction_content(" 👍 ").unwrap(), "👍");
    }

    #[test]
    fn validate_reaction_content_accepts_vs16_sequence() {
        // ❤️ = U+2764 + VS16（クイックリアクションで使われる形）
        assert_eq!(validate_reaction_content("❤️").unwrap(), "❤️");
    }

    #[test]
    fn validate_reaction_content_accepts_skin_tone_modifier() {
        assert!(validate_reaction_content("👍🏽").is_ok());
    }

    #[test]
    fn validate_reaction_content_accepts_zwj_sequence() {
        // 家族の ZWJ 結合絵文字
        assert!(validate_reaction_content("👨‍👩‍👧").is_ok());
    }

    #[test]
    fn validate_reaction_content_accepts_flag_sequence() {
        assert!(validate_reaction_content("🇯🇵").is_ok());
    }

    #[test]
    fn validate_reaction_content_rejects_plain_text() {
        assert!(validate_reaction_content("いいね").is_err());
        assert!(validate_reaction_content("nice").is_err());
    }

    #[test]
    fn validate_reaction_content_rejects_shortcode() {
        assert!(validate_reaction_content(":smile:").is_err());
    }

    #[test]
    fn validate_reaction_content_rejects_bare_digit_and_keycap_base() {
        // 単体の数字/#/* は emoji-data.txt 上 Emoji=Yes だが、キーキャップ結合が無ければ
        // 絵文字として認識しない（emojis crate は完全一致でしか通さない）
        assert!(validate_reaction_content("1").is_err());
        assert!(validate_reaction_content("#").is_err());
    }

    #[test]
    fn validate_reaction_content_accepts_keycap_sequence() {
        assert!(validate_reaction_content("1️⃣").is_ok());
    }

    #[test]
    fn validate_reaction_content_rejects_emoji_plus_text() {
        assert!(validate_reaction_content("🎉nice").is_err());
    }

    #[test]
    fn validate_reaction_content_rejects_empty() {
        assert!(validate_reaction_content("").is_err());
        assert!(validate_reaction_content("   ").is_err());
    }
}
