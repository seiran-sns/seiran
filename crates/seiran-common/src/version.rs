//! サーバー側のバージョン情報。フロントエンドとの互換性チェック
//! （`middleware::version_headers`、フロントエンドの`api/versionCompat.ts`）で使う。

/// このサーバーのバージョン（workspaceの`version`+`version_suffix`、`Cargo.toml`参照）。
/// `SEIRAN_VERSION_SUFFIX`は`build.rs`がワークスペースの`Cargo.toml`から読み取って
/// 払い出すビルド時環境変数（`version_suffix`はCargoの継承対象ではない独自キーのため）。
pub const SERVER_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), env!("SEIRAN_VERSION_SUFFIX"));

/// このサーバーが要求する、フロントエンドの最低バージョン。
/// 非互換な変更を加えて引き上げる際はユーザーへの相談が必要（`CLAUDE.md`参照）。
pub const SERVER_MIN_PEER_VERSION: &str = "0.2.11";
