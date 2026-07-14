//! seiran-federation-worker — 非同期ジョブ実行エンジン。
//!
//! バイナリは `seiran-server` が `--role worker`（または `all`）で起動する。
//!
//! `all` ロールでは `queue` は api ロールの `AppState.job_queue` と同一インスタンスを
//! 共有する（呼び出し元の `seiran-server` が一度だけ生成して両方に渡す）ため、
//! api 側から積んだジョブをプロセス内でこの worker がそのまま処理できる。
//! プロセスをまたいだキュー共有（split-role 構成）は Redis 統合（フェーズ 8）まで
//! 行わない点に注意（`--role worker` 単独起動時は自分専用のキューになる）。

use std::sync::Arc;

use sqlx::PgPool;

use seiran_common::ap::ApClient;
use seiran_common::queue::{InMemoryJobQueue, WorkerEngine};

/// ワーカーエンジンを起動し、ジョブを処理し続ける（常駐）。
/// `queue`/`pool`/`ap_client` は呼び出し元が生成した共有インスタンスを受け取る
/// （`all` ロールでは api/federation と同じキュー・DBプール・コネクションプールを
/// 再利用する）。
pub async fn run(queue: Arc<InMemoryJobQueue>, pool: PgPool, ap_client: Arc<ApClient>) {
    eprintln!("[federation-worker] 起動中...");

    let engine = WorkerEngine::new_with_db(queue, pool, ap_client);

    // デキュー・実行・リトライを永続的に回す
    engine.run().await;
}
