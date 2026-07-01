//! seiran-federation-worker — 非同期ジョブ実行エンジン。
//!
//! バイナリは `seiran-server` が `--role worker`（または `all`）で起動する。
//!
//! 現状は `InMemoryJobQueue` を使う独立プロセスとして常駐する。プロセスをまたいだ
//! キュー共有は Redis 統合（フェーズ 8）まで行わない。ただし `all` モードでは
//! api / inbox と同一プロセスで動くため、将来はプロセス内でキューを共有できる。

use std::sync::Arc;

use seiran_common::queue::{InMemoryJobQueue, WorkerEngine};

/// ワーカーエンジンを起動し、ジョブを処理し続ける（常駐）。
pub async fn run() {
    eprintln!("[federation-worker] 起動中...");

    let queue = Arc::new(InMemoryJobQueue::new());
    let engine = WorkerEngine::new(queue);

    // デキュー・実行・リトライを永続的に回す
    engine.run().await;
}
