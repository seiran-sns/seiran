//! ジョブキューモジュール
//!
//! # 構成
//! - `JobQueue` Trait: キューのインターフェース（traits.rs に定義済みの Job 型を使用）
//! - `InMemoryJobQueue`: Redis を使わない構成向けのオンメモリ実装（1プロセスで完結する `all` 専用）
//! - `RedisJobQueue`: プロセスをまたいでキューを共有する実装（split-role・無停止更新）
//! - `WorkerEngine`: ジョブのデキュー・実行・リトライを管理する実行エンジン

pub mod memory;
pub mod redis;
pub mod worker;

pub use memory::InMemoryJobQueue;
pub use redis::RedisJobQueue;
pub use worker::WorkerEngine;

use crate::traits::JobQueue;
use std::sync::Arc;

/// ジョブキューを用意する。`redis` があれば `RedisJobQueue`（全プロセスでキューを共有し、
/// プロセスを再起動・入れ替えても未処理のジョブとリトライ待ちが残る）、無ければ
/// `InMemoryJobQueue`（このプロセスの中だけ。停止すると未処理のジョブは消える）。
///
/// Redis を設定したのに繋がらないときは起動を失敗させる。InMemory に黙って切り替えると、
/// split-role では他プロセスにジョブが届かず、無停止更新では停止する側のジョブが消える。
pub async fn create_job_queue(redis_url: Option<&str>) -> Result<Arc<dyn JobQueue>, String> {
    match redis_url {
        Some(url) => {
            let queue = RedisJobQueue::connect(url)
                .await
                .map_err(|e| format!("Redis のジョブキューに接続できません: {}", e))?;
            tracing::info!("[seiran] JobQueue バックエンド: Redis");
            Ok(Arc::new(queue))
        }
        None => {
            tracing::info!("[seiran] JobQueue バックエンド: InMemory");
            Ok(Arc::new(InMemoryJobQueue::new()))
        }
    }
}
