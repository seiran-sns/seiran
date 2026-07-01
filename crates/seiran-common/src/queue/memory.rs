//! オンメモリ JobQueue 実装
//!
//! 開発・テスト環境向け。プロセス再起動でキューの内容は消えるが、
//! 外部依存ゼロで即座に動作確認できる。
//!
//! # 内部構造
//! - `tokio::sync::Mutex<BinaryHeap<PrioritizedJob>>`: 優先度付きヒープによるキュー
//! - `tokio::sync::Notify`: 新規ジョブ投入時に Worker を起こすための通知機構

use async_trait::async_trait;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

use crate::traits::{Job, JobQueue};

/// 優先度付きジョブエントリ
/// BinaryHeap は最大ヒープなので priority が大きいほど先に処理される
#[derive(Debug)]
struct PrioritizedJob {
    priority: i32,
    /// 投入順序（同一優先度内での FIFO 保証）
    sequence: u64,
    job: Job,
}

impl PartialEq for PrioritizedJob {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl Eq for PrioritizedJob {}

impl PartialOrd for PrioritizedJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrioritizedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // 優先度が高いほど先（最大ヒープ）
        // 同一優先度内では投入順（sequence が小さいほど先 → sequence の逆順比較）
        other
            .priority
            .cmp(&self.priority)
            .reverse()
            .then(other.sequence.cmp(&self.sequence).reverse())
    }
}

/// オンメモリ JobQueue の共有状態
struct InnerQueue {
    heap: BinaryHeap<PrioritizedJob>,
    sequence: u64,
}

/// オンメモリ実装の JobQueue
pub struct InMemoryJobQueue {
    inner: Arc<Mutex<InnerQueue>>,
    /// Worker 起動通知チャンネル
    pub notify: Arc<Notify>,
}

impl InMemoryJobQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(InnerQueue {
                heap: BinaryHeap::new(),
                sequence: 0,
            })),
            notify: Arc::new(Notify::new()),
        }
    }

    /// キューからジョブを1件デキューします（Worker が呼び出す）
    pub async fn dequeue(&self) -> Option<Job> {
        let mut inner = self.inner.lock().await;
        inner.heap.pop().map(|pj| pj.job)
    }

    /// 現在のキュー長を返します
    pub async fn len(&self) -> usize {
        self.inner.lock().await.heap.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.heap.is_empty()
    }

    /// 通知ハンドルを複製して返します（WorkerEngine が保持する用）
    pub fn notify_handle(&self) -> Arc<Notify> {
        self.notify.clone()
    }
}

impl Default for InMemoryJobQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl JobQueue for InMemoryJobQueue {
    async fn enqueue(&self, job: Job, priority: i32) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        inner.sequence += 1;
        let seq = inner.sequence;
        inner.heap.push(PrioritizedJob {
            priority,
            sequence: seq,
            job,
        });
        // Worker を起こす
        self.notify.notify_one();
        Ok(())
    }
}
