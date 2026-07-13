//! Misskey 実物のワイヤープロトコルに合わせた**追加**エンドポイント群（Phase 2）。
//!
//! 既存の `handlers::notes`/`handlers::users`/`handlers::follows` 等のカスタムAPIは
//! そのまま維持し、third-party の Misskey クライアント（Aria/Miria/ZonePane）が直接叩ける
//! パス・POSTオンリー規約・レスポンス形状のエンドポイントをこのモジュール配下に別途生やす。
//! 認証（`i` トークン）は `middleware::misskey_auth_bridge` が `Authorization` ヘッダーへ
//! 合成済みのため、ここでは通常の `extract_auth` をそのまま使う。
//!
//! 書き込み系（リアクション・リノート取消・フォロー）は既存ハンドラの関数を直接呼び出して
//! 副作用ロジック（AP/ATP配送等）を再利用し、レスポンスだけ Misskey 流（成功時 204 No
//! Content）に整形する。読み取り系（ノート・ユーザー）は Note/UserLite が別スキーマのため、
//! リポジトリ層から直接組み立てる。

pub mod endpoints;
pub mod types;

mod convert;
