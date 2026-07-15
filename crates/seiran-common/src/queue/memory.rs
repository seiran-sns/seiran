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
        // BinaryHeap は最大ヒープなので Greater と評価された方が先にポップされる。
        // 優先度が高いほど先、同一優先度内では sequence が小さい（先に投入された）方が先。
        self.priority
            .cmp(&other.priority)
            .then(other.sequence.cmp(&self.sequence))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用のダミージョブ。actor_id で個体を識別する。
    fn job(id: i64) -> Job {
        Job::ApDelivery { actor_id: id, kind: crate::traits::ApDeliveryKind::DeleteActor }
    }

    fn post_id_of(job: &Job) -> i64 {
        match job {
            Job::ApDelivery { actor_id, .. } => *actor_id,
            _ => panic!("テストでは ApDelivery のみ使用する"),
        }
    }

    #[tokio::test]
    async fn dequeue_returns_none_when_empty() {
        let q = InMemoryJobQueue::new();
        assert!(q.dequeue().await.is_none());
        assert!(q.is_empty().await);
        assert_eq!(q.len().await, 0);
    }

    #[tokio::test]
    async fn higher_priority_dequeued_first() {
        let q = InMemoryJobQueue::new();
        q.enqueue(job(1), 10).await.unwrap();
        q.enqueue(job(2), 100).await.unwrap();
        q.enqueue(job(3), 50).await.unwrap();

        assert_eq!(post_id_of(&q.dequeue().await.unwrap()), 2);
        assert_eq!(post_id_of(&q.dequeue().await.unwrap()), 3);
        assert_eq!(post_id_of(&q.dequeue().await.unwrap()), 1);
    }

    #[tokio::test]
    async fn same_priority_is_fifo() {
        let q = InMemoryJobQueue::new();
        for i in 1..=5 {
            q.enqueue(job(i), 10).await.unwrap();
        }
        for i in 1..=5 {
            assert_eq!(post_id_of(&q.dequeue().await.unwrap()), i, "同一優先度は投入順（FIFO）で取り出す");
        }
    }

    #[tokio::test]
    async fn mixed_priorities_keep_fifo_within_same_priority() {
        let q = InMemoryJobQueue::new();
        q.enqueue(job(1), 10).await.unwrap();
        q.enqueue(job(2), 50).await.unwrap();
        q.enqueue(job(3), 10).await.unwrap();
        q.enqueue(job(4), 50).await.unwrap();

        let order: Vec<i64> = [
            q.dequeue().await.unwrap(),
            q.dequeue().await.unwrap(),
            q.dequeue().await.unwrap(),
            q.dequeue().await.unwrap(),
        ]
        .iter()
        .map(post_id_of)
        .collect();
        assert_eq!(order, vec![2, 4, 1, 3]);
    }

    #[tokio::test]
    async fn enqueue_notifies_worker() {
        let q = InMemoryJobQueue::new();
        let notify = q.notify_handle();
        q.enqueue(job(1), 10).await.unwrap();
        // enqueue 済みなら notified() は即座に解決する（permit が立っている）
        tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified())
            .await
            .expect("enqueue 後に notify されていない");
    }

    #[tokio::test]
    async fn len_tracks_queue_size() {
        let q = InMemoryJobQueue::new();
        q.enqueue(job(1), 10).await.unwrap();
        q.enqueue(job(2), 10).await.unwrap();
        assert_eq!(q.len().await, 2);
        q.dequeue().await;
        assert_eq!(q.len().await, 1);
    }
}
