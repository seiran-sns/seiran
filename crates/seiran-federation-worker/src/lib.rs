//! seiran-federation-worker — 非同期ジョブ実行エンジン。
//!
//! バイナリは `seiran-server` が `--role worker`（または `all`）で起動する。
//!
//! `queue` は `Arc<dyn JobQueue>` を受け取るため、バックエンドを問わず動作する。
//! キューは `REDIS_URL` があれば全プロセスで共有する `RedisJobQueue`、無ければ
//! このプロセス内の `InMemoryJobQueue`（`seiran_common::Messaging` 参照）。

use seiran_common::queue::WorkerEngine;
use seiran_common::{CancellationToken, WorkerConfig};

/// ワーカーエンジンを起動し、`shutdown` が発火して実行中のジョブが終わるまでジョブを処理する。
/// `config` の共有インスタンスは呼び出し元が生成したものを受け取る（`all` ロールでは
/// api/federation と同じキュー・DBプール・コネクションプール・ATPコミットサービスを使う）。
pub async fn run(config: WorkerConfig, shutdown: CancellationToken) {
    tracing::info!("[federation-worker] 起動中...");
    WorkerEngine::new_with_db(config).run(shutdown).await;
}
