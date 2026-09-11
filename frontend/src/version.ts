/** このフロントエンドのバージョン（`package.json`の`version`、ビルド時に埋め込み）。 */
export const FRONTEND_VERSION = __FRONTEND_VERSION__;

/**
 * このフロントエンドが要求する、サーバーの最低バージョン。
 * 非互換な変更を加えて引き上げる際はユーザーへの相談が必要（`CLAUDE.md`参照）。
 */
export const FRONTEND_MIN_PEER_VERSION = "0.1.0";
