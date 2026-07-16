//! seiran-atp-repo — AT Protocol Firehose リスナー。
//!
//! Bluesky Firehose に WebSocket で接続し、フォロー済みアクターの新規ポストを
//! リアルタイムで DB に保存する常駐処理。
//!
//! バイナリは `seiran-server` が `--role firehose`（または `all`）で起動する。

use std::sync::Arc;

use sqlx::PgPool;
use seiran_common::StreamHub;

pub mod firehose;

/// Firehose リスナーを起動する（常駐）。
///
/// `redis_url`があれば、複数インスタンス起動時のJetstream接続排他制御（リーダー選出）を
/// 行う。`is_monolith`はRedis未使用時・通信失敗時のフェイルオープン/フェイルクローズを
/// 決める（`true`＝`all`ロール、`false`＝`firehose`単独ロール。Doc3 §14.2参照）。
pub async fn run(
    pool: PgPool,
    http: Arc<reqwest::Client>,
    stream_hub: Arc<StreamHub>,
    redis_url: Option<String>,
    is_monolith: bool,
) {
    tracing::info!("[seiran-atp-repo] Firehose リスナーを起動します。");
    firehose::run(pool, http, stream_hub, redis_url, is_monolith).await;
}
