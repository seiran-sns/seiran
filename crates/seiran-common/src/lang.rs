//! フロントの i18n が対応する言語コードの共通定義。表示言語設定
//! （`account:languagePreference`）とポスト言語プロパティ（`posts.language`、
//! Bsky配送の`langs`にのみ反映、`docs/protocols.md`参照）の両方がこのリストを許可値として使う。

pub const SUPPORTED_LANGUAGES: [&str; 7] = ["ja", "en", "zh", "ko", "es", "de", "fr"];

pub fn is_supported_language(language: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&language)
}
