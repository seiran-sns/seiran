//! WorkerEngine: ジョブのデキュー・実行・リトライを管理する実行エンジン
//!
//! # 動作フロー
//! 1. `InMemoryJobQueue::notify` を待機
//! 2. ジョブをデキュー
//! 3. 対応するジョブハンドラを呼び出し
//! 4. 失敗時は指数バックオフでリトライキューへ再投入
//!
//! # 優先度定数
//! ```
//! const PRIORITY_CRITICAL : i32 = 100;  // ATP リポジトリコミット
//! const PRIORITY_HIGH     : i32 =  50;  // AP 配送（アウトバウンド）
//! const PRIORITY_NORMAL   : i32 =  10;  // インバウンド処理、メタデータ解決
//! const PRIORITY_LOW      : i32 =   1;  // 過去ログ同期
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

use crate::ap::ApClient;
use crate::jobs;
use crate::queue::InMemoryJobQueue;
use crate::traits::{Job, JobQueue};

/// ジョブの優先度定数
pub mod priority {
    pub const CRITICAL: i32 = 100;
    pub const HIGH: i32 = 50;
    pub const NORMAL: i32 = 10;
    pub const LOW: i32 = 1;
}

/// リトライ設定
struct RetryConfig {
    max_attempts: u32,
    /// 初回待機時間（この値を底として指数バックオフ）
    base_delay_ms: u64,
    /// 最大待機時間（上限キャップ）
    max_delay_ms: u64,
}

/// AP 配送ジョブ（`Job::ApDelivery`）に必要な設定。
/// 起動時に `seiran-server` が secrets / 環境変数から一度だけ組み立てて注入する。
#[derive(Clone)]
pub struct DeliveryConfig {
    pub local_domain: String,
    pub ap_private_key_pem: Option<String>,
    pub ap_public_key_pem: Option<String>,
}

/// ジョブ実行コンテキスト（ハンドラに渡す）
pub struct JobContext {
    pub queue: Arc<dyn JobQueue>,
    /// ドメイン単位の同時実行制限（ActorHistorySync 等で使用）
    pub domain_semaphores: Arc<tokio::sync::Mutex<HashMap<String, Arc<Semaphore>>>>,
    /// アクター単位の排他実行制御（AtpRepositoryPublish 等で使用）
    pub actor_semaphores: Arc<tokio::sync::Mutex<HashMap<i64, Arc<Semaphore>>>>,
    /// DB 接続プール（フェーズ4以降のジョブハンドラが使用）
    pub db_pool: Option<sqlx::PgPool>,
    /// AP クライアント（HTTP クライアントと公開鍵キャッシュを保持）
    pub ap_client: Arc<ApClient>,
    /// AP 配送設定（ApDelivery ジョブが使用）
    pub delivery: Option<DeliveryConfig>,
}

impl JobContext {
    /// `ap_client` は呼び出し元（`seiran-server`）が起動時に一度だけ生成した共有インスタンスを
    /// 受け取る。ここでローカルに `reqwest::Client` を生成すると、api/federation ロールと
    /// 別のコネクションプールになってしまうため禁止（`docs/coding_rules.md` 禁止事項 #8）。
    pub fn new(queue: Arc<dyn JobQueue>, ap_client: Arc<ApClient>) -> Self {
        Self {
            queue,
            domain_semaphores: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            actor_semaphores: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            db_pool: None,
            ap_client,
            delivery: None,
        }
    }

    pub fn with_db_pool(mut self, pool: sqlx::PgPool) -> Self {
        self.db_pool = Some(pool);
        self
    }

    pub fn with_delivery_config(mut self, config: DeliveryConfig) -> Self {
        self.delivery = Some(config);
        self
    }

    /// ドメイン単位のセマフォを取得または生成します（最大並列数: 2）
    pub async fn get_domain_semaphore(&self, domain: &str) -> Arc<Semaphore> {
        let mut map = self.domain_semaphores.lock().await;
        map.entry(domain.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(2)))
            .clone()
    }

    /// アクター単位の排他セマフォ（最大並列数: 1）を取得または生成します
    pub async fn get_actor_semaphore(&self, actor_id: i64) -> Arc<Semaphore> {
        let mut map = self.actor_semaphores.lock().await;
        map.entry(actor_id)
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    }
}

/// WorkerEngine: ジョブキューを監視し、ジョブを実行するバックグラウンドエンジン
pub struct WorkerEngine {
    queue: Arc<InMemoryJobQueue>,
    ctx: Arc<JobContext>,
}

impl WorkerEngine {
    pub fn new(queue: Arc<InMemoryJobQueue>, ap_client: Arc<ApClient>) -> Self {
        let ctx = Arc::new(JobContext::new(queue.clone(), ap_client));
        Self { queue, ctx }
    }

    pub fn new_with_db(
        queue: Arc<InMemoryJobQueue>,
        pool: sqlx::PgPool,
        ap_client: Arc<ApClient>,
        delivery: DeliveryConfig,
    ) -> Self {
        let ctx = Arc::new(
            JobContext::new(queue.clone(), ap_client)
                .with_db_pool(pool)
                .with_delivery_config(delivery),
        );
        Self { queue, ctx }
    }

    /// バックグラウンドワーカーループを起動します
    /// このメソッドは `tokio::spawn` で呼び出してください
    pub async fn run(self) {
        let notify = self.queue.notify_handle();
        eprintln!("[WorkerEngine] ジョブワーカー起動");

        loop {
            // ジョブが投入されるまで待機（既にキューに残っていれば即処理）
            if self.queue.len().await == 0 {
                notify.notified().await;
            }

            while let Some(job) = self.queue.dequeue().await {
                let ctx = self.ctx.clone();
                let queue = self.queue.clone();

                // ジョブごとに別タスクで実行（ブロッキングを防ぐ）
                tokio::spawn(async move {
                    execute_with_retry(job, ctx, queue, 0).await;
                });
            }
        }
    }
}

/// ジョブを実行し、失敗時は指数バックオフでリトライします
async fn execute_with_retry(
    job: Job,
    ctx: Arc<JobContext>,
    queue: Arc<InMemoryJobQueue>,
    attempt: u32,
) {
    let config = retry_config_for(&job);
    let job_name = job_name(&job);

    eprintln!(
        "[Worker] 実行開始: {} (attempt {}/{})",
        job_name,
        attempt + 1,
        config.max_attempts
    );

    // 所有権をクローンして渡すことで、ライフタイム参照による非Send問題を解消
    let result = dispatch_job(job.clone(), ctx.clone()).await;

    match result {
        Ok(()) => {
            eprintln!("[Worker] 完了: {}", job_name);
        }
        Err(e) if attempt + 1 < config.max_attempts => {
            // 指数バックオフ + ジッター（0〜1秒）
            let jitter_ms = {
                use argon2::password_hash::rand_core::{OsRng, RngCore};
                let mut rng = OsRng;
                rng.next_u32() as u64 % 1000
            };
            let wait = Duration::from_millis(backoff_delay_ms(&config, attempt) + jitter_ms);

            eprintln!(
                "[Worker] 失敗: {} - {} → {}ms後にリトライ (attempt {})",
                job_name, e, wait.as_millis(), attempt + 1
            );

            // ヘルパー関数を介して再スケジュールすることで、直接の非同期再帰によるコンパイラの混乱を防ぐ
            spawn_retry(job, ctx, queue, attempt + 1, wait);
        }
        Err(e) => {
            eprintln!(
                "[Worker] 最大リトライ数に達しました（破棄）: {} - {}",
                job_name, e
            );
        }
    }
}

/// 指数バックオフの待機時間（ジッター抜き）を計算する。
/// `base_delay_ms * 2^attempt` を `max_delay_ms` でクランプする。
fn backoff_delay_ms(config: &RetryConfig, attempt: u32) -> u64 {
    config
        .base_delay_ms
        .saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX))
        .min(config.max_delay_ms)
}

/// リトライを遅延実行するためのタスクを起動する同期的なヘルパー関数
fn spawn_retry(
    job: Job,
    ctx: Arc<JobContext>,
    queue: Arc<InMemoryJobQueue>,
    attempt: u32,
    delay: Duration,
) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        execute_with_retry(job, ctx, queue, attempt).await;
    });
}


/// ジョブ種別ごとにハンドラを呼び出します（所有権を受け取る）
async fn dispatch_job(job: Job, ctx: Arc<JobContext>) -> Result<(), String> {
    match job {
        Job::ActorHistorySync { ap_uri, at_did } => {
            jobs::actor_history_sync::handle(ap_uri, at_did, ctx).await
        }
        Job::ApDelivery { actor_id, kind } => {
            jobs::ap_delivery::handle(actor_id, kind, ctx).await
        }
        Job::InboundActivityProcess { raw_activity } => {
            jobs::inbound_activity_process::handle(raw_activity, ctx).await
        }
        Job::ActorMetadataResolve { actor_id } => {
            jobs::actor_metadata_resolve::handle(actor_id, ctx).await
        }
        Job::AtpRepositoryPublish { actor_id, commit_type } => {
            jobs::atp_repository_publish::handle(actor_id, commit_type, ctx).await
        }
        Job::BskyVideoPoll { media_file_id } => {
            jobs::bsky_video_poll::handle(media_file_id, ctx).await
        }
    }
}

/// ジョブの人間可読な名前を返します（ログ用）
fn job_name(job: &Job) -> &'static str {
    match job {
        Job::ActorHistorySync { .. } => "ActorHistorySync",
        Job::ApDelivery { .. } => "ApDelivery",
        Job::InboundActivityProcess { .. } => "InboundActivityProcess",
        Job::ActorMetadataResolve { .. } => "ActorMetadataResolve",
        Job::AtpRepositoryPublish { .. } => "AtpRepositoryPublish",
        Job::BskyVideoPoll { .. } => "BskyVideoPoll",
    }
}

/// ジョブ種別ごとのリトライ設定を返します
fn retry_config_for(job: &Job) -> RetryConfig {
    match job {
        Job::ActorHistorySync { .. } => RetryConfig {
            max_attempts: 3,
            base_delay_ms: 1000, // 1s → 2s → 4s
            max_delay_ms: 30_000,
        },
        Job::ApDelivery { .. } => RetryConfig {
            max_attempts: 10,
            base_delay_ms: 5000, // 5s → 10s → ... → max
            max_delay_ms: 3_600_000, // 最大1時間
        },
        Job::InboundActivityProcess { .. } => RetryConfig {
            max_attempts: 3,
            base_delay_ms: 2000,
            max_delay_ms: 60_000,
        },
        Job::ActorMetadataResolve { .. } => RetryConfig {
            max_attempts: 3,
            base_delay_ms: 1000,
            max_delay_ms: 30_000,
        },
        Job::AtpRepositoryPublish { .. } => RetryConfig {
            max_attempts: 5,
            base_delay_ms: 500,
            max_delay_ms: 10_000,
        },
        Job::BskyVideoPoll { .. } => RetryConfig {
            // 固定3秒間隔・最大10回（=最大30秒）。base=max にすることで
            // 指数バックオフの式が初回からmax_delay_msにクランプされ実質固定間隔になる。
            max_attempts: 10,
            base_delay_ms: 3000,
            max_delay_ms: 3000,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_exponentially() {
        let config = RetryConfig { max_attempts: 10, base_delay_ms: 1000, max_delay_ms: 30_000 };
        assert_eq!(backoff_delay_ms(&config, 0), 1000);
        assert_eq!(backoff_delay_ms(&config, 1), 2000);
        assert_eq!(backoff_delay_ms(&config, 2), 4000);
        assert_eq!(backoff_delay_ms(&config, 3), 8000);
    }

    #[test]
    fn backoff_is_clamped_to_max_delay() {
        let config = RetryConfig { max_attempts: 10, base_delay_ms: 5000, max_delay_ms: 60_000 };
        assert_eq!(backoff_delay_ms(&config, 10), 60_000);
    }

    #[test]
    fn backoff_with_equal_base_and_max_is_fixed_interval() {
        // BskyVideoPoll の「固定3秒間隔」設定が成立していること
        let config = RetryConfig { max_attempts: 10, base_delay_ms: 3000, max_delay_ms: 3000 };
        for attempt in 0..10 {
            assert_eq!(backoff_delay_ms(&config, attempt), 3000);
        }
    }

    #[test]
    fn backoff_does_not_overflow_on_huge_attempt() {
        let config = RetryConfig { max_attempts: 100, base_delay_ms: 1000, max_delay_ms: 60_000 };
        // 2^attempt が u64 を溢れる領域でもパニックせず max にクランプされる
        assert_eq!(backoff_delay_ms(&config, 63), 60_000);
        assert_eq!(backoff_delay_ms(&config, 64), 60_000);
        assert_eq!(backoff_delay_ms(&config, 200), 60_000);
    }
}
