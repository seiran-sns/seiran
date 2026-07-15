//! seiran-federation-worker — 非同期ジョブ実行エンジン。
//!
//! バイナリは `seiran-server` が `--role worker`（または `all`）で起動する。
//!
//! `queue` は `Arc<dyn JobQueue>` を受け取るため、バックエンドを問わず動作する。
//! `all` ロールでは api ロールの `AppState.job_queue`（InMemoryJobQueue）と同一
//! インスタンスを共有し、split-role 構成では `REDIS_URL` が設定されていれば
//! `RedisJobQueue` を共有する（`seiran_common::create_job_queue` 参照）。

use std::sync::Arc;

use sqlx::PgPool;

use seiran_common::ap::ApClient;
use seiran_common::queue::WorkerEngine;
use seiran_common::{DeliveryConfig, InboxContext, JobQueue};

/// ワーカーエンジンを起動し、ジョブを処理し続ける（常駐）。
/// `queue`/`pool`/`ap_client` は呼び出し元が生成した共有インスタンスを受け取る
/// （`all` ロールでは api/federation と同じキュー・DBプール・コネクションプールを
/// 再利用する）。`delivery` は AP 配送ジョブ用の設定（ドメイン・AP 鍵）。
/// `inbox` は `Job::InboundActivityProcess`（AP Inbox 受信処理）に必要な設定。
pub async fn run(
    queue: Arc<dyn JobQueue>,
    pool: PgPool,
    ap_client: Arc<ApClient>,
    delivery: DeliveryConfig,
    inbox: Option<InboxContext>,
) {
    tracing::info!("[federation-worker] 起動中...");

    let engine = WorkerEngine::new_with_db(queue, pool, ap_client, delivery, inbox);

    // デキュー・実行・リトライを永続的に回す
    engine.run().await;
}
