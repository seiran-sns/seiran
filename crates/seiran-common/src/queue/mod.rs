//! ジョブキューモジュール
//!
//! # 構成
//! - `JobQueue` Trait: キューのインターフェース（traits.rs に定義済みの Job 型を使用）
//! - `InMemoryJobQueue`: モノリスモード（`--role all`）向けのオンメモリ実装
//! - `RedisJobQueue`: split-role 構成（プロセスをまたいでキューを共有する）向けの実装
//! - `WorkerEngine`: ジョブのデキュー・実行・リトライを管理する実行エンジン

pub mod memory;
pub mod redis;
pub mod worker;

pub use memory::InMemoryJobQueue;
pub use redis::RedisJobQueue;
pub use worker::WorkerEngine;

use std::sync::Arc;
use crate::traits::JobQueue;

/// 環境変数 `REDIS_URL` の名前（複数箇所で参照するため定数化）。
const REDIS_URL_ENV: &str = "REDIS_URL";

/// ロールに応じて適切な JobQueue 実装を生成する。
///
/// - `is_monolith == true`（`--role all`）: 常に `InMemoryJobQueue`。同一プロセス内で
///   api/worker が動くため、キュー共有に外部ミドルウェアは不要（`REDIS_URL` の有無は見ない）。
/// - `is_monolith == false`（split-role: api / federation / worker を別プロセスで起動）:
///   `REDIS_URL` が設定されていれば `RedisJobQueue`（プロセス間でキューを共有できる）。
///   未設定なら `InMemoryJobQueue` にフォールバックするが、この場合 enqueue したジョブは
///   同一プロセス内でしか処理されない（他プロセスの Worker には届かない既知の制約。
///   split-role 構成で実際に Worker を分離したいなら `REDIS_URL` の設定が必須）。
pub async fn create_job_queue(is_monolith: bool) -> Arc<dyn JobQueue> {
    if is_monolith {
        tracing::info!("[seiran] JobQueue バックエンド: InMemory（モノリスモード）");
        return Arc::new(InMemoryJobQueue::new());
    }

    match std::env::var(REDIS_URL_ENV) {
        Ok(url) if !url.is_empty() => match RedisJobQueue::connect(&url).await {
            Ok(q) => {
                tracing::info!("[seiran] JobQueue バックエンド: Redis（split-role分散モード）");
                Arc::new(q)
            }
            Err(e) => {
                tracing::error!(
                    "[seiran] Redis接続に失敗しました（{}）。InMemoryにフォールバックしますが、\
                     split-role構成では他プロセスのWorkerにジョブが届きません: {}",
                    url, e
                );
                Arc::new(InMemoryJobQueue::new())
            }
        },
        _ => {
            tracing::warn!(
                "[seiran] {} 未設定のためInMemoryを使用します。split-role構成では \
                 他プロセスのWorkerにジョブが届かない既知の制約があります",
                REDIS_URL_ENV
            );
            Arc::new(InMemoryJobQueue::new())
        }
    }
}
