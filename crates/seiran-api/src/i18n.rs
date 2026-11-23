//! サーバーが出す文言（メール・OGP）と、その言語の決定。
//!
//! 文言は `locales/<lang>/<namespace>.json` に言語・用途ごとに置き、バイナリへ埋め込む。形式は
//! フロントの `frontend/src/i18n/locales/<lang>/*.json` と同じ（入れ子のキー、差し込みは
//! `{{name}}`）にして、将来ユーザーが作った言語ファイルを配るときにフロントと同じ形で扱えるように
//! している。
//!
//! 言語は表示言語（[`SUPPORTED_DISPLAY_LANGUAGES`]）のどれか。決め方は
//! [`request_language`] と [`stored_language`] を参照。

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::http::{header::ACCEPT_LANGUAGE, HeaderMap};
use seiran_common::SUPPORTED_DISPLAY_LANGUAGES;
use serde_json::Value;

/// どの手がかりからも言語が決まらないときの言語（フロントの `fallbackLng` と同じ）。
pub const FALLBACK_LANGUAGE: &str = "en";

/// 文言の用途。`locales/<lang>/` の下のファイル名に対応する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// メールの件名・本文（`mail.json`、`crate::mailer`）。
    Mail,
    /// OGP の `<title>`・`og:title`・既定の `og:description`（`ogp.json`、`handlers::ogp`）。
    Ogp,
}

macro_rules! locale {
    ($lang:literal) => {
        (
            $lang,
            include_str!(concat!("../locales/", $lang, "/mail.json")),
            include_str!(concat!("../locales/", $lang, "/ogp.json")),
        )
    };
}

/// （言語, `mail.json`, `ogp.json`）。
const LOCALE_SOURCES: [(&str, &str, &str); SUPPORTED_DISPLAY_LANGUAGES.len()] = [
    locale!("ja"),
    locale!("en"),
    locale!("zh-Hant"),
    locale!("zh-Hans"),
    locale!("ko"),
    locale!("es"),
    locale!("de"),
    locale!("fr"),
];

static MESSAGES: LazyLock<HashMap<(Namespace, &'static str), Value>> = LazyLock::new(|| {
    LOCALE_SOURCES
        .iter()
        .flat_map(|(lang, mail, ogp)| {
            [
                (Namespace::Mail, *lang, *mail, "mail"),
                (Namespace::Ogp, *lang, *ogp, "ogp"),
            ]
        })
        .map(|(ns, lang, source, file)| {
            let value = serde_json::from_str(source).unwrap_or_else(|e| {
                panic!("locales/{lang}/{file}.json が JSON として読めない: {e}")
            });
            ((ns, lang), value)
        })
        .collect()
});

/// 対応する表示言語なら、その `'static` な言語コードを返す。
fn supported(language: &str) -> Option<&'static str> {
    SUPPORTED_DISPLAY_LANGUAGES
        .iter()
        .copied()
        .find(|l| *l == language)
}

/// ブラウザの操作から決める言語（メール・OGP）。`preferred`（フロントが送る、画面に出ている
/// 言語。ATP クライアントからの操作では `users.language_preference`）が対応言語ならそれ、
/// 無いか非対応なら `Accept-Language` を表示言語へ丸めたもの、それでも決まらなければ
/// [`FALLBACK_LANGUAGE`]。
pub fn request_language(preferred: Option<&str>, headers: &HeaderMap) -> &'static str {
    preferred
        .and_then(supported)
        .or_else(|| accept_language(headers))
        .unwrap_or(FALLBACK_LANGUAGE)
}

/// 保存しておいた言語（`at_migration_requests.language`・`users.language_preference`）で、
/// 後から非同期に送るメールの言語。NULL・非対応なら [`FALLBACK_LANGUAGE`]。
pub fn stored_language(stored: Option<&str>) -> &'static str {
    stored.and_then(supported).unwrap_or(FALLBACK_LANGUAGE)
}

/// `Accept-Language` の言語範囲を q 値の高い順（同じなら書かれた順）に見て、最初に表示言語へ
/// 丸められたもの。
fn accept_language(headers: &HeaderMap) -> Option<&'static str> {
    let header = headers.get(ACCEPT_LANGUAGE)?.to_str().ok()?;
    let mut ranges: Vec<(f32, &str)> = header
        .split(',')
        .filter_map(|item| {
            let mut parts = item.split(';');
            let tag = parts.next()?.trim();
            let q = parts
                .find_map(|p| p.trim().strip_prefix("q="))
                .map_or(Some(1.0), |q| q.trim().parse::<f32>().ok())?;
            (q > 0.0 && !tag.is_empty()).then_some((q, tag))
        })
        .collect();
    // 安定ソートなので、q 値が同じ範囲は書かれた順のまま残る。
    ranges.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranges
        .into_iter()
        .find_map(|(_, tag)| normalize_language_tag(tag))
}

/// 言語タグを表示言語へ丸める（フロントの `normalizeDetectedLanguage` と同じ規則）。中国語は
/// 用字（`Hant`/`Hans`）か地域（台湾・香港・マカオは繁體）で分け、どちらも無ければ简体字。
fn normalize_language_tag(tag: &str) -> Option<&'static str> {
    let lower = tag.to_ascii_lowercase();
    let mut subtags = lower.split(['-', '_']);
    let primary = subtags.next()?;
    if primary != "zh" {
        return supported(primary);
    }
    let rest: Vec<&str> = subtags.collect();
    let traditional = if rest.contains(&"hant") {
        true
    } else if rest.contains(&"hans") {
        false
    } else {
        rest.iter().any(|s| matches!(*s, "tw" | "hk" | "mo"))
    };
    Some(if traditional { "zh-Hant" } else { "zh-Hans" })
}

fn lookup<'a>(messages: &'a Value, key: &str) -> Option<&'a str> {
    key.split('.')
        .try_fold(messages, |node, part| node.get(part))?
        .as_str()
}

/// `namespace` の `key`（`verifyEmail.body` のような `.` 区切り）の文言に `vars` を差し込む。
/// その言語にキーが無ければ [`FALLBACK_LANGUAGE`] の文言を使う（言語ファイルの欠けでメールが
/// 送れなくならないように）。
pub fn text(namespace: Namespace, language: &str, key: &str, vars: &[(&str, &str)]) -> String {
    let template = MESSAGES
        .get(&(namespace, language))
        .and_then(|m| lookup(m, key))
        .or_else(|| lookup(&MESSAGES[&(namespace, FALLBACK_LANGUAGE)], key))
        .unwrap_or(key);
    interpolate(template, vars)
}

/// `{{name}}` を `vars` の値に置き換える。1回の走査で置き換えるので、差し込んだ値の中の
/// `{{...}}` は置き換えない（ユーザー名や URL から別の差し込みが起きないように）。
/// `vars` に無い名前はそのまま残す。
fn interpolate(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end].trim();
        match vars.iter().find(|(k, _)| *k == name) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn headers(accept_language: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT_LANGUAGE, accept_language.parse().unwrap());
        h
    }

    /// 文字列の葉までのキーと、その文言に含まれる差し込み名の一覧。
    fn leaves(value: &Value, prefix: &str, out: &mut BTreeSet<(String, BTreeSet<String>)>) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    let key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    leaves(v, &key, out);
                }
            }
            Value::String(s) => {
                let names = s
                    .split("{{")
                    .skip(1)
                    .filter_map(|p| p.split_once("}}").map(|(n, _)| n.trim().to_string()))
                    .collect();
                out.insert((prefix.to_string(), names));
            }
            other => panic!("{prefix}: 文字列でも入れ子でもない値 {other}"),
        }
    }

    #[test]
    fn every_language_has_the_same_keys_and_placeholders() {
        for ns in [Namespace::Mail, Namespace::Ogp] {
            let mut reference = BTreeSet::new();
            leaves(&MESSAGES[&(ns, "ja")], "", &mut reference);
            assert!(!reference.is_empty(), "{ns:?}");
            for lang in SUPPORTED_DISPLAY_LANGUAGES {
                let mut got = BTreeSet::new();
                leaves(&MESSAGES[&(ns, lang)], "", &mut got);
                assert_eq!(
                    got, reference,
                    "{ns:?}: {lang} のキーか差し込み名が ja と違う"
                );
            }
        }
    }

    #[test]
    fn explicit_language_wins_and_is_validated() {
        let h = headers("de-DE,de;q=0.9");
        assert_eq!(request_language(Some("ko"), &h), "ko");
        assert_eq!(request_language(Some("xx"), &h), "de");
        assert_eq!(request_language(None, &h), "de");
        assert_eq!(request_language(None, &HeaderMap::new()), "en");
    }

    #[test]
    fn accept_language_follows_q_values() {
        assert_eq!(
            accept_language(&headers("it;q=1, fr;q=0.5, ja;q=0.8")),
            Some("ja")
        );
        assert_eq!(accept_language(&headers("it, pt-BR")), None);
        assert_eq!(accept_language(&headers("*, es;q=0.1")), Some("es"));
        assert_eq!(accept_language(&headers("ja;q=0, en;q=0.2")), Some("en"));
    }

    #[test]
    fn chinese_is_split_by_script_then_region() {
        assert_eq!(normalize_language_tag("zh-TW"), Some("zh-Hant"));
        assert_eq!(normalize_language_tag("zh-Hant-CN"), Some("zh-Hant"));
        assert_eq!(normalize_language_tag("zh-Hans-HK"), Some("zh-Hans"));
        assert_eq!(normalize_language_tag("zh"), Some("zh-Hans"));
        assert_eq!(normalize_language_tag("zh_hk"), Some("zh-Hant"));
        assert_eq!(normalize_language_tag("EN-us"), Some("en"));
    }

    #[test]
    fn stored_language_falls_back_to_english() {
        assert_eq!(stored_language(Some("zh-Hant")), "zh-Hant");
        assert_eq!(stored_language(Some("zh")), "en");
        assert_eq!(stored_language(None), "en");
    }

    #[test]
    fn interpolation_does_not_rescan_inserted_values() {
        assert_eq!(
            interpolate("a {{x}} b {{y}} {{z}}", &[("x", "{{y}}"), ("y", "Y")]),
            "a {{y}} b Y {{z}}"
        );
        assert_eq!(interpolate("open {{x", &[("x", "1")]), "open {{x");
    }

    #[test]
    fn text_renders_every_language() {
        for lang in SUPPORTED_DISPLAY_LANGUAGES {
            let body = text(
                Namespace::Mail,
                lang,
                "verifyEmail.body",
                &[("url", "https://example.test/v")],
            );
            assert!(body.contains("https://example.test/v"), "{lang}");
            assert!(!body.contains("{{"), "{lang}");
        }
        assert_eq!(
            text(Namespace::Mail, "ja", "no.such.key", &[]),
            "no.such.key"
        );
    }
}
