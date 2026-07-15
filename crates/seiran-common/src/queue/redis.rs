//! Redis バックエンドの JobQueue 実装
//!
//! split-role 構成（api / federation / worker を別プロセス・別コンテナで起動する構成）で
//! プロセスをまたいでキューを共有するために使う。モノリスモード（`--role all`）では
//! 同一プロセス内で完結するため `InMemoryJobQueue` で十分であり、これは不要
//! （`docs/`「キューバックエンド選択方針」参照）。
//!
//! # データ構造
//! - `seiran:jobs:ready`（Sorted Set）: 実行可能なジョブ。score は優先度＋投入順で
//!   計算し（[`ready_score`]）、`BZPOPMIN` で最小 score（＝最優先）から取り出す。
//! - `seiran:jobs:delayed`（Sorted Set）: リトライ待ちのジョブ。score は
//!   実行可能になる unix time (ms)。バックグラウンドで期限が来たものを ready へ昇格する。
//! - `seiran:jobs:seq`（文字列, INCR）: 投入順を表す単調増加シーケンス番号。
//!
//! メンバー（ZSET の要素）はジョブ本体を JSON にした [`Envelope`] そのもの。
//! 別途ハッシュにペイロードを保持する方式より単純で、孤立キーが残るリスクもない。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use redis::aio::{ConnectionManager, MultiplexedConnection};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::traits::{Job, JobQueue, QueuedJob};

/// デフォルトのキー接頭辞（本番用）。テストは衝突を避けるため別の接頭辞を使う
/// （`RedisJobQueue::connect_with_prefix`、`#[cfg(test)]` 限定）。
const DEFAULT_KEY_PREFIX: &str = "seiran:jobs";

/// BZPOPMIN のタイムアウト（秒）。delayed → ready 昇格のポーリング粒度も兼ねる。
const BLOCK_TIMEOUT_SECS: f64 = 1.0;
/// 1回のポーリングで昇格させる delayed ジョブの最大件数。
const PROMOTE_BATCH_LIMIT: i64 = 200;

/// 期限が来た delayed ジョブを ready へ原子的に移す Lua スクリプト。
/// 複数の Worker プロセスが同時にポーリングしても二重昇格しないよう、
/// ZREM の戻り値（実際に削除できたか）で移動権を確定させてから ZADD する。
const PROMOTE_DUE_LUA: &str = r#"
local due = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[1], 'LIMIT', 0, ARGV[2])
local promoted = 0
for _, member in ipairs(due) do
    if redis.call('ZREM', KEYS[1], member) == 1 then
        local data = cjson.decode(member)
        local score = -(data.priority) * 1e15 + data.seq
        redis.call('ZADD', KEYS[2], score, member)
        promoted = promoted + 1
    end
end
return promoted
"#;

#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    seq: u64,
    priority: i32,
    attempt: u32,
    job: Job,
}

/// ready ZSET 用のスコアを計算する。
/// BZPOPMIN は最小 score から取り出すため、優先度が高い（数値が大きい）ほど、
/// また同一優先度内では投入順（seq）が早いほど、score が小さくなるようにする。
fn ready_score(priority: i32, seq: u64) -> f64 {
    -(priority as f64) * 1e15 + seq as f64
}

pub struct RedisJobQueue {
    /// enqueue / promote 等の非ブロッキング操作用（複数箇所から共有・クローンして使う）。
    conn: ConnectionManager,
    /// BZPOPMIN 専用の占有コネクション。ブロッキングコマンドは Redis 側で同一コネクション上の
    /// 後続コマンドをせき止めるため、`conn`（enqueue 等が使う）とは分離が必須。
    blocking_conn: Mutex<MultiplexedConnection>,
    promote_script: redis::Script,
    ready_key: String,
    delayed_key: String,
    seq_key: String,
}

impl RedisJobQueue {
    /// `redis_url` は `redis://host:port/db` 形式。
    pub async fn connect(redis_url: &str) -> Result<Self, String> {
        Self::connect_with_prefix(redis_url, DEFAULT_KEY_PREFIX).await
    }

    /// キー接頭辞を指定して接続する。本番は常に [`DEFAULT_KEY_PREFIX`] 固定（`connect` 経由）。
    /// テストが並列実行時にキー空間を衝突させないためだけに接頭辞を変えられるようにしている。
    async fn connect_with_prefix(redis_url: &str, prefix: &str) -> Result<Self, String> {
        let client = redis::Client::open(redis_url)
            .map_err(|e| format!("Redis接続URLが不正です: {}", e))?;
        let conn = ConnectionManager::new(client.clone())
            .await
            .map_err(|e| format!("Redis接続に失敗しました: {}", e))?;
        let blocking_conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| format!("Redis接続に失敗しました（blocking用）: {}", e))?;
        Ok(Self {
            conn,
            blocking_conn: Mutex::new(blocking_conn),
            promote_script: redis::Script::new(PROMOTE_DUE_LUA),
            ready_key: format!("{}:ready", prefix),
            delayed_key: format!("{}:delayed", prefix),
            seq_key: format!("{}:seq", prefix),
        })
    }

    async fn next_seq(&self) -> Result<u64, String> {
        let mut conn = self.conn.clone();
        let seq: i64 = conn.incr(&self.seq_key, 1i64).await.map_err(|e| e.to_string())?;
        Ok(seq as u64)
    }

    async fn push_ready(&self, job: Job, priority: i32, attempt: u32) -> Result<(), String> {
        let seq = self.next_seq().await?;
        let member = serde_json::to_string(&Envelope { seq, priority, attempt, job })
            .map_err(|e| format!("ジョブのシリアライズに失敗しました: {}", e))?;
        let score = ready_score(priority, seq);
        let mut conn = self.conn.clone();
        let _: () = conn.zadd(&self.ready_key, member, score).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn push_delayed(&self, job: Job, priority: i32, attempt: u32, delay: Duration) -> Result<(), String> {
        let seq = self.next_seq().await?;
        let member = serde_json::to_string(&Envelope { seq, priority, attempt, job })
            .map_err(|e| format!("ジョブのシリアライズに失敗しました: {}", e))?;
        let ready_at_ms = chrono::Utc::now().timestamp_millis() + delay.as_millis() as i64;
        let mut conn = self.conn.clone();
        let _: () = conn
            .zadd(&self.delayed_key, member, ready_at_ms as f64)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 期限が来た delayed ジョブを ready へ昇格させる。
    async fn promote_due(&self) -> Result<(), String> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut conn = self.conn.clone();
        let _promoted: i64 = self
            .promote_script
            .key(&self.delayed_key)
            .key(&self.ready_key)
            .arg(now_ms)
            .arg(PROMOTE_BATCH_LIMIT)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[async_trait]
impl JobQueue for RedisJobQueue {
    async fn enqueue(&self, job: Job, priority: i32) -> Result<(), String> {
        self.push_ready(job, priority, 0).await
    }

    async fn enqueue_retry(&self, job: Job, priority: i32, attempt: u32, delay: Duration) -> Result<(), String> {
        self.push_delayed(job, priority, attempt, delay).await
    }

    async fn dequeue_blocking(&self) -> QueuedJob {
        loop {
            if let Err(e) = self.promote_due().await {
                tracing::error!("[RedisJobQueue] 遅延ジョブの昇格に失敗しました: {}", e);
            }

            let popped: Option<(String, String, f64)> = {
                let mut conn = self.blocking_conn.lock().await;
                match conn.bzpopmin(&self.ready_key, BLOCK_TIMEOUT_SECS).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("[RedisJobQueue] BZPOPMIN に失敗しました: {} → 1秒後にリトライ", e);
                        drop(conn);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        None
                    }
                }
            };

            let Some((_key, member, _score)) = popped else {
                continue;
            };

            match serde_json::from_str::<Envelope>(&member) {
                Ok(env) => return QueuedJob { job: env.job, priority: env.priority, attempt: env.attempt },
                Err(e) => {
                    tracing::error!("[RedisJobQueue] ジョブのデシリアライズに失敗しました（破棄）: {} raw={}", e, member);
                    continue;
                }
            }
        }
    }
}

/// `Arc<RedisJobQueue>` へも `JobQueue` を実装しておくと、呼び出し元が
/// 具象型のまま複数箇所（WorkerEngine / API ハンドラ）で共有しやすい。
#[async_trait]
impl JobQueue for Arc<RedisJobQueue> {
    async fn enqueue(&self, job: Job, priority: i32) -> Result<(), String> {
        (**self).enqueue(job, priority).await
    }

    async fn enqueue_retry(&self, job: Job, priority: i32, attempt: u32, delay: Duration) -> Result<(), String> {
        (**self).enqueue_retry(job, priority, attempt, delay).await
    }

    async fn dequeue_blocking(&self) -> QueuedJob {
        (**self).dequeue_blocking().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_score_higher_priority_sorts_first() {
        // BZPOPMIN は最小 score から取り出すため、優先度が高いほど score は小さくなる
        assert!(ready_score(100, 1) < ready_score(50, 1));
        assert!(ready_score(50, 1) < ready_score(10, 1));
        assert!(ready_score(10, 1) < ready_score(1, 1));
    }

    #[test]
    fn ready_score_same_priority_is_fifo() {
        assert!(ready_score(10, 1) < ready_score(10, 2));
        assert!(ready_score(10, 2) < ready_score(10, 100));
    }

    #[test]
    fn ready_score_priority_always_dominates_sequence() {
        // seq がどれだけ大きくても、優先度が1でも高ければ score は必ず小さくなる
        // （優先度差 1 は 1e15、想定される総ジョブ数よりはるかに大きいマージン）
        assert!(ready_score(11, 1_000_000_000) < ready_score(10, 1));
    }

    /// Redis サーバーが必要な結合テスト。`SEIRAN_TEST_REDIS_URL` が未設定なら skip する。
    /// テストは並列実行されるため、テストごとに一意なキー接頭辞を使いキー空間を分離する
    /// （同一 Redis インスタンスを複数テストで共有しても互いに干渉しない）。
    /// 例: `SEIRAN_TEST_REDIS_URL=redis://127.0.0.1:16399 cargo test -p seiran-common redis::`
    async fn connect_for_test() -> Option<RedisJobQueue> {
        let url = std::env::var("SEIRAN_TEST_REDIS_URL").ok()?;
        let prefix = format!("seiran:test:{}", uuid::Uuid::new_v4());
        Some(
            RedisJobQueue::connect_with_prefix(&url, &prefix)
                .await
                .expect("テスト用Redisへの接続に失敗"),
        )
    }

    fn job(id: i64) -> Job {
        Job::ApDelivery { actor_id: id, kind: crate::traits::ApDeliveryKind::DeleteActor }
    }

    fn actor_id_of(job: &Job) -> i64 {
        match job {
            Job::ApDelivery { actor_id, .. } => *actor_id,
            _ => panic!("テストでは ApDelivery のみ使用する"),
        }
    }

    #[tokio::test]
    async fn enqueue_then_dequeue_round_trip() {
        let Some(q) = connect_for_test().await else {
            tracing::warn!("SEIRAN_TEST_REDIS_URL 未設定のため skip");
            return;
        };
        q.enqueue(job(9001), 10).await.unwrap();
        let qj = tokio::time::timeout(Duration::from_secs(3), q.dequeue_blocking())
            .await
            .expect("dequeue_blocking がタイムアウトした");
        assert_eq!(actor_id_of(&qj.job), 9001);
        assert_eq!(qj.priority, 10);
        assert_eq!(qj.attempt, 0);
    }

    #[tokio::test]
    async fn higher_priority_dequeued_first() {
        let Some(q) = connect_for_test().await else {
            tracing::warn!("SEIRAN_TEST_REDIS_URL 未設定のため skip");
            return;
        };
        q.enqueue(job(1), 10).await.unwrap();
        q.enqueue(job(2), 100).await.unwrap();
        q.enqueue(job(3), 50).await.unwrap();

        let a = tokio::time::timeout(Duration::from_secs(3), q.dequeue_blocking()).await.unwrap();
        let b = tokio::time::timeout(Duration::from_secs(3), q.dequeue_blocking()).await.unwrap();
        let c = tokio::time::timeout(Duration::from_secs(3), q.dequeue_blocking()).await.unwrap();
        assert_eq!(actor_id_of(&a.job), 2);
        assert_eq!(actor_id_of(&b.job), 3);
        assert_eq!(actor_id_of(&c.job), 1);
    }

    #[tokio::test]
    async fn enqueue_retry_becomes_ready_after_delay() {
        let Some(q) = connect_for_test().await else {
            tracing::warn!("SEIRAN_TEST_REDIS_URL 未設定のため skip");
            return;
        };
        q.enqueue_retry(job(42), 50, 2, Duration::from_millis(500)).await.unwrap();

        // まだ delayed 側にあり、ready からは即座には取り出せないはず
        let too_early = tokio::time::timeout(Duration::from_millis(300), q.dequeue_blocking()).await;
        assert!(too_early.is_err(), "delay 前に取り出せてしまった");

        // delay 経過後、promote_due のポーリングで ready へ昇格し取り出せる
        let qj = tokio::time::timeout(Duration::from_secs(3), q.dequeue_blocking())
            .await
            .expect("delay 後も dequeue_blocking がタイムアウトした");
        assert_eq!(actor_id_of(&qj.job), 42);
        assert_eq!(qj.priority, 50);
        assert_eq!(qj.attempt, 2);
    }
}
