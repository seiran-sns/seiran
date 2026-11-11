//! リモートFedi投稿のCW（閲覧注意）警告文をseiran Web UI表示用のサニタイズ済みHTMLへ変換する。
//!
//! APの`summary`は実装によって中身が異なる。Mastodonはプレーンテキストを、Misskeyは生のMFMを
//! そのまま送ってくる（本文の`content`はMFMを送信側でHTMLに変換済みなのに、`summary`は
//! 変換しない）。そのため警告文には`<center>`・`<small>`のようなMFMのHTML風タグや、
//! `$[ruby 親 ルビ]`・`**太字**`のようなMFM記法がそのまま入っている。これを文字のまま見せると
//! 記法が露出するので、本文の`content_html`と同じallowlistを通したHTMLにする。
//!
//! 元の`posts.content_warning`は変更しない（Misskey互換APIは生のMFMを返すのが正しく、
//! 削除して再編集でも元の文字列が要るため）。変換は応答の組み立て時に行う。

use std::sync::LazyLock;

use regex::Regex;

use crate::jobs::inbound_activity_process::sanitize_html_allowlist;

/// 警告文を変換する必要があるか。記法を含まない大多数の警告文は変換せず、フロントの
/// プレーンテキスト表示に任せる。
fn needs_conversion(cw: &str) -> bool {
    cw.contains('<') || cw.contains("$[") || cw.contains("**") || cw.contains("~~")
}

/// リモートFedi投稿の警告文を、表示用のサニタイズ済みHTMLへ変換する。変換すべき記法を
/// 含まない場合は`None`。
pub fn fedi_content_warning_html(cw: &str) -> Option<String> {
    let cw = cw.trim_end();
    if !needs_conversion(cw) {
        return None;
    }
    let escaped = escape_non_tag_lt(cw);
    let mut parser = MfmParser {
        chars: escaped.chars().collect(),
        pos: 0,
    };
    let converted = parser.parse_inline(false).0;
    let converted = convert_decorations(&converted);
    let with_breaks = converted.replace("\r\n", "\n").replace('\n', "<br>");
    Some(sanitize_html_allowlist(&with_breaks))
}

/// タグとして解釈できる`<`の形（`<name>`・`</name>`・`<name attr>`・`<name/>`、タグ名は英字始まり）。
static TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^</?[A-Za-z][A-Za-z0-9]*(?:\s[^<>]*)?/?>").unwrap());

/// タグの形をしていない`<`だけを`&lt;`にする。警告文は基本的にプレーンテキストなので、
/// `<ご報告>`・`(◍>ᴗ<◍)`・`<3`のような文字としての`<`をHTMLパーサーに渡すと、後続の
/// 文字列ごとタグとして飲み込まれて消えてしまう。`&`はエスケープしない（既にエンティティ化
/// して送ってくる実装があり、裸の`&`はHTMLパーサーがそのまま文字として扱うため）。
fn escape_non_tag_lt(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(m) = TAG_RE.find(tail) {
            out.push_str(m.as_str());
            rest = &tail[m.end()..];
        } else {
            out.push_str("&lt;");
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// MFMの`$[関数 中身]`と`<plain>`を処理する再帰下降パーサー。
struct MfmParser {
    chars: Vec<char>,
    pos: usize,
}

impl MfmParser {
    fn starts_with(&self, s: &str) -> bool {
        let mut i = self.pos;
        for c in s.chars() {
            match self.chars.get(i) {
                Some(&x) if x.eq_ignore_ascii_case(&c) => i += 1,
                _ => return false,
            }
        }
        true
    }

    /// 現在位置から読み進める。`in_fn`なら対応する`]`で止め、それを消費して`(中身, true)`を
    /// 返す。閉じる`]`が無いまま末尾に達したら`(中身, false)`。
    fn parse_inline(&mut self, in_fn: bool) -> (String, bool) {
        let mut out = String::new();
        while self.pos < self.chars.len() {
            if in_fn && self.chars[self.pos] == ']' {
                self.pos += 1;
                return (out, true);
            }
            if self.starts_with("<plain>") {
                out.push_str(&self.parse_plain());
                continue;
            }
            if self.starts_with("$[") {
                if let Some(html) = self.parse_fn() {
                    out.push_str(&html);
                    continue;
                }
            }
            out.push(self.chars[self.pos]);
            self.pos += 1;
        }
        (out, false)
    }

    /// `<plain>…</plain>`の中身はMFMとして解釈しない。後段の装飾変換にも拾われないよう、
    /// 記法に使う文字を数値文字参照にしておく（サニタイズで元の文字に戻る）。
    fn parse_plain(&mut self) -> String {
        let start = self.pos + "<plain>".len();
        let mut i = start;
        while i < self.chars.len() {
            self.pos = i;
            if self.starts_with("</plain>") {
                let inner: String = self.chars[start..i].iter().collect();
                self.pos = i + "</plain>".len();
                return inner
                    .replace('*', "&#42;")
                    .replace('~', "&#126;")
                    .replace('$', "&#36;");
            }
            i += 1;
        }
        // 閉じタグが無ければ`<plain>`を文字として扱う（タグは後段のサニタイズで除去される）。
        self.pos = start;
        "<plain>".to_string()
    }

    /// `$[name.args 中身]`。形が崩れていれば位置を戻して`None`（呼び出し側が文字として扱う）。
    fn parse_fn(&mut self) -> Option<String> {
        let start = self.pos;
        self.pos += 2;
        let name_start = self.pos;
        while self
            .chars
            .get(self.pos)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
        {
            self.pos += 1;
        }
        let name: String = self.chars[name_start..self.pos].iter().collect();
        if self.chars.get(self.pos) == Some(&'.') {
            while self
                .chars
                .get(self.pos)
                .is_some_and(|c| !c.is_whitespace() && *c != ']')
            {
                self.pos += 1;
            }
        }
        if name.is_empty() || self.chars.get(self.pos) != Some(&' ') {
            self.pos = start;
            return None;
        }
        self.pos += 1;
        let (inner, closed) = self.parse_inline(true);
        if !closed {
            self.pos = start;
            return None;
        }
        Some(if name == "ruby" {
            ruby_html(&inner)
        } else {
            // 回転・点滅等の効果は再現せず中身だけ見せる（記法が露出しないことを優先）。
            inner
        })
    }
}

/// Misskeyの`$[ruby 親文字 ルビ]`と同じく、最初の空白で親文字とルビに分ける。
fn ruby_html(inner: &str) -> String {
    match inner.split_once(' ') {
        Some((base, rt)) => {
            let rt = rt.split(' ').next().unwrap_or_default();
            format!("<ruby>{base}<rp>(</rp><rt>{rt}</rt><rp>)</rp></ruby>")
        }
        None => inner.to_string(),
    }
}

static BOLD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
static STRIKE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"~~(.+?)~~").unwrap());

/// MFMの`**太字**`・`~~打ち消し~~`をタグにする。
fn convert_decorations(s: &str) -> String {
    let s = BOLD_RE.replace_all(s, "<b>$1</b>");
    STRIKE_RE.replace_all(&s, "<s>$1</s>").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_not_converted() {
        assert_eq!(fedi_content_warning_html("ネタバレ注意"), None);
    }

    #[test]
    fn ruby_function_becomes_ruby_tag() {
        let out = fedi_content_warning_html("おれと$[ruby 三代鬼徹 コイツ]の呪い").unwrap();
        assert_eq!(
            out,
            "おれと<ruby>三代鬼徹<rp>(</rp><rt>コイツ</rt><rp>)</rp></ruby>の呪い"
        );
    }

    #[test]
    fn other_functions_keep_only_content_even_when_nested() {
        let out = fedi_content_warning_html("$[sparkle $[x2.speed=1s **自己紹介**]]！").unwrap();
        assert_eq!(out, "<b>自己紹介</b>！");
    }

    #[test]
    fn allowed_tags_are_kept_and_newlines_become_br() {
        let out = fedi_content_warning_html("<center>スタッフ募集<small>の予告</small></center>\n")
            .unwrap();
        assert_eq!(out, "<center>スタッフ募集<small>の予告</small></center>");
        let out = fedi_content_warning_html("<small>1行目\n2行目</small>").unwrap();
        assert_eq!(out, "<small>1行目<br>2行目</small>");
    }

    #[test]
    fn disallowed_tags_are_stripped_and_scripts_removed() {
        let out = fedi_content_warning_html("<span>a</span><script>alert(1)</script>").unwrap();
        assert_eq!(out, "a");
        let out = fedi_content_warning_html(r#"<b onclick="x()">b</b>"#).unwrap();
        assert_eq!(out, "<b>b</b>");
    }

    #[test]
    fn literal_lt_is_kept_as_text() {
        let out = fedi_content_warning_html("<ご報告>").unwrap();
        assert_eq!(out, "&lt;ご報告&gt;");
        let out = fedi_content_warning_html("(◍>ᴗ<◍) <3 >⩊<♡").unwrap();
        assert_eq!(out, "(◍&gt;ᴗ&lt;◍) &lt;3 &gt;⩊&lt;♡");
    }

    #[test]
    fn unclosed_function_is_left_as_text() {
        let out = fedi_content_warning_html("$[x2 閉じない").unwrap();
        assert_eq!(out, "$[x2 閉じない");
    }

    #[test]
    fn plain_tag_suppresses_mfm() {
        let out = fedi_content_warning_html("<plain>**そのまま** $[x2 a]</plain>").unwrap();
        assert_eq!(out, "**そのまま** $[x2 a]");
    }
}
