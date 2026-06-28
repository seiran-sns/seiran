//! ジョブキューモジュール
//!
//! # 構成
//! - `JobQueue` Trait: キューのインターフェース（traits.rs に定義済みの Job 型を使用）
//! - `InMemoryJobQueue`: 開発・テスト用のオンメモリ実装
//! - `WorkerEngine`: ジョブのデキュー・実行・リトライを管理する実行エンジン
//!
//! # 将来拡張
//! - `RedisJobQueue`: 本番スケール用の Redis バックエンド（フェーズ8で実装予定）

pub mod memory;
pub mod worker;

pub use memory::InMemoryJobQueue;
pub use worker::WorkerEngine;

use std::sync::Arc;
use crate::traits::JobQueue;

/// 環境変数に基づいて適切な JobQueue 実装を生成します。
///
/// - `JOB_QUEUE_BACKEND=memory`（デフォルト）: InMemoryJobQueue
/// - `JOB_QUEUE_BACKEND=redis`: RedisJobQueue（将来実装）
pub fn create_job_queue() -> Arc<dyn JobQueue> {
    let backend = std::env::var("JOB_QUEUE_BACKEND").unwrap_or_else(|_| "memory".to_string());

    match backend.as_str() {
        _ => {
            eprintln!("[seiran] JobQueue バックエンド: InMemory（開発モード）");
            Arc::new(InMemoryJobQueue::new())
        }
    }
}
