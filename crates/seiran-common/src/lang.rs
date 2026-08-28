//! フロントの i18n が対応する言語コードの共通定義。
//!
//! ポスト言語プロパティ（`posts.language`、Bsky配送の`langs`にのみ反映、
//! `docs/protocols.md`参照）は[`SUPPORTED_LANGUAGES`]（7言語）を許可値として使う。
//!
//! 表示言語設定（`users.language_preference`）は[`SUPPORTED_DISPLAY_LANGUAGES`]
//! （8言語）を許可値として使う。中国語のみ`zh-Hant`（繁體中文）/`zh-Hans`（简体中文）の
//! バリエーションを持ち、ポスト言語側の`zh`と1対2で対応する。表示言語からポスト言語の
//! デフォルト値を決める際はフロント側（`i18n/index.ts`のpostLanguageBase）で
//! `zh-Hant`/`zh-Hans`→`zh`へ丸める。

pub const SUPPORTED_LANGUAGES: [&str; 7] = ["ja", "en", "zh", "ko", "es", "de", "fr"];

pub fn is_supported_language(language: &str) -> bool {
    SUPPORTED_LANGUAGES.contains(&language)
}

pub const SUPPORTED_DISPLAY_LANGUAGES: [&str; 8] =
    ["ja", "en", "zh-Hant", "zh-Hans", "ko", "es", "de", "fr"];

pub fn is_supported_display_language(language: &str) -> bool {
    SUPPORTED_DISPLAY_LANGUAGES.contains(&language)
}
