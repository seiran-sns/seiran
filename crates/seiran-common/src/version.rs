//! サーバー側のバージョン情報。フロントエンドとの互換性チェック
//! （`middleware::version_headers`、フロントエンドの`api/versionCompat.ts`）で使う。

/// このサーバーのバージョン（workspaceの`version`、`Cargo.toml`参照）。
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// このサーバーが要求する、フロントエンドの最低バージョン。
/// 非互換な変更を加えて引き上げる際はユーザーへの相談が必要（`CLAUDE.md`参照）。
pub const SERVER_MIN_PEER_VERSION: &str = "0.1.0";
