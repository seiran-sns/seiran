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
pub async fn run(pool: PgPool, http: Arc<reqwest::Client>, stream_hub: Arc<StreamHub>) {
    eprintln!("[seiran-atp-repo] Firehose リスナーを起動します。");
    firehose::run(pool, http, stream_hub).await;
}
