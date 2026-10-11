//! プロセスの外とつながる配信路（ジョブキュー・WebSocket へのリアルタイム配信・
//! subscribeRepos のコミットイベント）と、プロセスの停止の合図をまとめたもの。
//!
//! 1プロセスで1回だけ作り、そのプロセスのすべてのロールで共有する。`REDIS_URL` があれば
//! 3つの配信路をすべて Redis でプロセス間につなぎ、無ければプロセス内で完結させる。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::atp::AtpCommitEvent;
use crate::event_bus::EventBus;
use crate::streaming::StreamHub;
use crate::traits::JobQueue;

#[derive(Clone)]
pub struct Messaging {
    pub job_queue: Arc<dyn JobQueue>,
    pub stream_hub: Arc<StreamHub>,
    pub atp_events: Arc<EventBus<AtpCommitEvent>>,
    /// 発火したら、新しい仕事を受け付けずに手持ちの処理を終えて止まる。
    pub shutdown: CancellationToken,
}

impl Messaging {
    /// `redis_url` が設定されていて繋がらないときは失敗する（黙ってプロセス内に切り替えると、
    /// 他のプロセスにジョブやイベントが届かなくなるため）。
    pub async fn connect(
        redis_url: Option<&str>,
        shutdown: CancellationToken,
    ) -> Result<Self, String> {
        let job_queue = crate::queue::create_job_queue(redis_url).await?;
        let redis = redis_url
            .map(redis::Client::open)
            .transpose()
            .map_err(|e| format!("REDIS_URL が不正です: {}", e))?;
        let stream_hub = match &redis {
            Some(client) => StreamHub::with_redis(client)
                .await
                .map_err(|e| format!("Redis（リアルタイム配信）に接続できません: {}", e))?,
            None => StreamHub::new(),
        };
        let atp_events = AtpCommitEvent::bus(redis.as_ref())
            .await
            .map_err(|e| format!("Redis（subscribeRepos）に接続できません: {}", e))?;
        Ok(Self {
            job_queue,
            stream_hub: Arc::new(stream_hub),
            atp_events: Arc::new(atp_events),
            shutdown,
        })
    }

    /// プロセス内で完結する配信路（テスト用）。
    pub fn local() -> Self {
        Self {
            job_queue: Arc::new(crate::queue::InMemoryJobQueue::new()),
            stream_hub: Arc::new(StreamHub::new()),
            atp_events: Arc::new(EventBus::local(16)),
            shutdown: CancellationToken::new(),
        }
    }
}
