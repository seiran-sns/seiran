//! WorkerEngine: ジョブのデキュー・実行・リトライを管理する実行エンジン
//!
//! # 動作フロー
//! 1. `JobQueue::dequeue_blocking` で実行可能なジョブを待機・取得（バックエンド非依存）
//! 2. グローバル並列数セマフォの permit を取得してから別タスクで実行
//! 3. 対応するジョブハンドラを呼び出し
//! 4. 失敗時は `JobQueue::enqueue_retry` で指数バックオフ後の再投入をキューに委ねる
//!    （リトライ待ち状態はキュー側が保持する。Redis 実装ならプロセス再起動を跨いで残る）
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
use crate::repository::{ActorRepository, FollowRepository, NotificationRepository, PostRepository, ReactionRepository};
use crate::streaming::StreamHub;
use crate::traits::{Job, JobQueue, QueuedJob};

/// ジョブの優先度定数
pub mod priority {
    pub const CRITICAL: i32 = 100;
    pub const HIGH: i32 = 50;
    pub const NORMAL: i32 = 10;
    pub const LOW: i32 = 1;
}

/// WorkerEngine が同時実行するジョブ数のデフォルト上限。
/// キューに大量のジョブが溜まっていた状態から復帰した際、一斉に spawn して
/// リソースを食い潰す（サンダリングハード）のを防ぐ。
const DEFAULT_MAX_CONCURRENT_JOBS: usize = 32;

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

/// インバウンド AP アクティビティ処理ジョブ（`Job::InboundActivityProcess`）に必要な設定。
/// `federation-inbox` ロールが起動時に一度だけ組み立てて注入する
/// （`all` ロールでは埋め込み Worker が api/federation と同じ `stream_hub` を共有する）。
#[derive(Clone)]
pub struct InboxContext {
    pub actor_repo: Arc<dyn ActorRepository>,
    pub follow_repo: Arc<dyn FollowRepository>,
    pub post_repo: Arc<dyn PostRepository>,
    pub reaction_repo: Arc<dyn ReactionRepository>,
    /// 通知（フォロー・リアクション等）の永続化リポジトリ。
    pub notification_repo: Arc<dyn NotificationRepository>,
    pub local_domain: String,
    pub ap_private_key_pem: String,
    /// リアルタイム更新（#37）の共有ストリーミングハブ。standalone Worker では
    /// 接続クライアントの居ない空ハブになる（`Role::Firehose` と同じ扱い）。
    pub stream_hub: Arc<StreamHub>,
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
    /// インバウンド AP アクティビティ処理設定（InboundActivityProcess ジョブが使用）
    pub inbox: Option<InboxContext>,
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
            inbox: None,
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

    pub fn with_inbox_context(mut self, inbox: InboxContext) -> Self {
        self.inbox = Some(inbox);
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

/// WorkerEngine: ジョブキューを監視し、ジョブを実行するバックグラウンドエンジン。
/// `queue` は `JobQueue` トレイトオブジェクトのため、InMemory / Redis どちらの
/// バックエンドでも同一コードで動作する。
pub struct WorkerEngine {
    queue: Arc<dyn JobQueue>,
    ctx: Arc<JobContext>,
    max_concurrent: usize,
}

impl WorkerEngine {
    pub fn new(queue: Arc<dyn JobQueue>, ap_client: Arc<ApClient>) -> Self {
        let ctx = Arc::new(JobContext::new(queue.clone(), ap_client));
        Self { queue, ctx, max_concurrent: DEFAULT_MAX_CONCURRENT_JOBS }
    }

    pub fn new_with_db(
        queue: Arc<dyn JobQueue>,
        pool: sqlx::PgPool,
        ap_client: Arc<ApClient>,
        delivery: DeliveryConfig,
        inbox: Option<InboxContext>,
    ) -> Self {
        let mut ctx_builder = JobContext::new(queue.clone(), ap_client)
            .with_db_pool(pool)
            .with_delivery_config(delivery);
        if let Some(inbox) = inbox {
            ctx_builder = ctx_builder.with_inbox_context(inbox);
        }
        let ctx = Arc::new(ctx_builder);
        Self { queue, ctx, max_concurrent: DEFAULT_MAX_CONCURRENT_JOBS }
    }

    /// 同時実行数の上限を変更する（既定は `DEFAULT_MAX_CONCURRENT_JOBS`）。
    pub fn with_max_concurrent(mut self, max_concurrent: usize) -> Self {
        self.max_concurrent = max_concurrent.max(1);
        self
    }

    /// バックグラウンドワーカーループを起動します
    /// このメソッドは `tokio::spawn` で呼び出してください
    pub async fn run(self) {
        tracing::info!("[WorkerEngine] ジョブワーカー起動 (最大並列数: {})", self.max_concurrent);
        let semaphore = Arc::new(Semaphore::new(self.max_concurrent));

        loop {
            let queued = self.queue.dequeue_blocking().await;
            let ctx = self.ctx.clone();
            let queue = self.queue.clone();

            // permit 取得をブロッキングで待つことで、実行中ジョブ数を上限内に保つ
            // （キューが一斉に溜まっていた場合のサンダリングハード対策）。
            let permit = Arc::clone(&semaphore)
                .acquire_owned()
                .await
                .expect("semaphore は閉じられない");

            tokio::spawn(async move {
                execute_with_retry(queued, ctx, queue).await;
                drop(permit);
            });
        }
    }
}

/// ジョブを実行し、失敗時は指数バックオフでリトライキューへ再投入します。
async fn execute_with_retry(queued: QueuedJob, ctx: Arc<JobContext>, queue: Arc<dyn JobQueue>) {
    let QueuedJob { job, priority, attempt } = queued;
    let config = retry_config_for(&job);
    let job_name = job_name(&job);

    tracing::info!(
        "[Worker] 実行開始: {} (attempt {}/{})",
        job_name,
        attempt + 1,
        config.max_attempts
    );

    // 所有権をクローンして渡すことで、ライフタイム参照による非Send問題を解消
    let result = dispatch_job(job.clone(), ctx.clone()).await;

    match result {
        Ok(()) => {
            tracing::info!("[Worker] 完了: {}", job_name);
        }
        Err(e) if attempt + 1 < config.max_attempts => {
            // 指数バックオフ + ジッター（0〜1秒）
            let jitter_ms = {
                use argon2::password_hash::rand_core::{OsRng, RngCore};
                let mut rng = OsRng;
                rng.next_u32() as u64 % 1000
            };
            let wait = Duration::from_millis(backoff_delay_ms(&config, attempt) + jitter_ms);

            tracing::error!(
                "[Worker] 失敗: {} - {} → {}ms後にリトライ (attempt {})",
                job_name, e, wait.as_millis(), attempt + 1
            );

            if let Err(enqueue_err) = queue.enqueue_retry(job, priority, attempt + 1, wait).await {
                tracing::error!(
                    "[Worker] リトライ再投入失敗（ジョブは失われました）: {} - {}",
                    job_name, enqueue_err
                );
            }
        }
        Err(e) => {
            tracing::error!(
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
        Job::ProxyFollowSync { target_actor_id, want_follow } => {
            jobs::proxy_follow_sync::handle(target_actor_id, want_follow, ctx).await
        }
        Job::AccountWithdrawUnfollowAll { actor_id, username } => {
            jobs::account_withdraw_unfollow_all::handle(actor_id, username, ctx).await
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
        Job::ProxyFollowSync { .. } => "ProxyFollowSync",
        Job::AccountWithdrawUnfollowAll { .. } => "AccountWithdrawUnfollowAll",
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
        Job::ProxyFollowSync { .. } => RetryConfig {
            max_attempts: 10,
            base_delay_ms: 5000, // ApDelivery と同様、リモートAP配送のため長めに構える
            max_delay_ms: 3_600_000,
        },
        Job::AccountWithdrawUnfollowAll { .. } => RetryConfig {
            max_attempts: 10,
            base_delay_ms: 5000, // ApDelivery/ProxyFollowSyncと同様、リモート配送を含むため長めに構える
            max_delay_ms: 3_600_000,
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

    #[tokio::test]
    async fn engine_processes_job_via_dyn_queue() {
        // WorkerEngine が Arc<dyn JobQueue> の背後で InMemory/Redis どちらでも
        // 動作することの最小回帰テスト（InMemory で代表）。
        use crate::ap::ApClient;
        use crate::queue::InMemoryJobQueue;

        let queue: Arc<dyn JobQueue> = Arc::new(InMemoryJobQueue::new());
        let http = Arc::new(reqwest::Client::new());
        let ap_client = Arc::new(ApClient::new(http));
        let engine = WorkerEngine::new(Arc::clone(&queue), ap_client).with_max_concurrent(2);

        queue
            .enqueue(Job::InboundActivityProcess { raw_activity: "{}".into() }, priority::NORMAL)
            .await
            .unwrap();

        let handle = tokio::spawn(engine.run());
        // InboundActivityProcess は未実装スタブで即 Ok を返すので短時間で完了するはず
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.abort();
    }
}
