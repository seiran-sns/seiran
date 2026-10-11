//! プロセスをまたいで配るブロードキャスト（WebSocket へのリアルタイム配信・
//! subscribeRepos のコミットイベント）。
//!
//! 購読者（WebSocket 接続）はどれか1つのプロセスにつながっている。イベントを起こした
//! プロセスと購読者のプロセスが違うと（split-role の worker が取り込んだ投稿、無停止更新で
//! 新旧2プロセスが重なっている間のコミットなど）、プロセス内の broadcast だけでは届かない。
//! Redis があれば、イベントを Redis の Pub/Sub に流し、各プロセス（自分自身を含む）が購読して
//! 手元の購読者へ配る。Redis が無ければプロセス内の broadcast だけで配る。
//!
//! 送信は1本のタスクに通して Redis へ流すので、同じプロセスから出たイベントの順序は保たれる。
//! Redis への送信に失敗したイベントは手元の購読者にだけ配る。

use std::time::Duration;

use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};

/// Redis の購読が切れたときに張り直すまでの間隔。
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(3);

pub struct EventBus<T> {
    local: broadcast::Sender<T>,
    /// Redis 経由で配るときの送信キュー。`None` ならプロセス内だけで配る。
    outbound: Option<mpsc::UnboundedSender<T>>,
}

impl<T> EventBus<T>
where
    T: Clone + Serialize + DeserializeOwned + Send + 'static,
{
    /// プロセス内だけで配るバス。
    pub fn local(capacity: usize) -> Self {
        let (local, _) = broadcast::channel(capacity);
        Self {
            local,
            outbound: None,
        }
    }

    /// Redis の `channel` を通して全プロセスへ配るバス。購読と送信のタスクを起動する。
    pub async fn with_redis(
        capacity: usize,
        client: &redis::Client,
        channel: &'static str,
    ) -> Result<Self, redis::RedisError> {
        let (local, _) = broadcast::channel(capacity);
        let publisher = redis::aio::ConnectionManager::new(client.clone()).await?;
        let (outbound, rx) = mpsc::unbounded_channel();
        tokio::spawn(run_publisher(publisher, channel, rx, local.clone()));
        tokio::spawn(run_subscriber(client.clone(), channel, local.clone()));
        Ok(Self {
            local,
            outbound: Some(outbound),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<T> {
        self.local.subscribe()
    }

    /// イベントを送出する（購読者がいなくてもエラーにしない）。
    pub fn publish(&self, event: T) {
        match &self.outbound {
            Some(outbound) => {
                if let Err(mpsc::error::SendError(event)) = outbound.send(event) {
                    let _ = self.local.send(event);
                }
            }
            None => {
                let _ = self.local.send(event);
            }
        }
    }
}

async fn run_publisher<T: Serialize>(
    mut conn: redis::aio::ConnectionManager,
    channel: &'static str,
    mut rx: mpsc::UnboundedReceiver<T>,
    local: broadcast::Sender<T>,
) {
    while let Some(event) = rx.recv().await {
        let payload = match serde_json::to_string(&event) {
            Ok(payload) => payload,
            Err(e) => {
                tracing::error!("[event_bus] {} のシリアライズに失敗: {}", channel, e);
                continue;
            }
        };
        if let Err(e) = redis::AsyncCommands::publish::<_, _, ()>(&mut conn, channel, payload).await
        {
            tracing::error!(
                "[event_bus] {} の Redis 送信に失敗（このプロセスの購読者にだけ配ります）: {}",
                channel,
                e
            );
            let _ = local.send(event);
        }
    }
}

async fn run_subscriber<T: DeserializeOwned>(
    client: redis::Client,
    channel: &'static str,
    local: broadcast::Sender<T>,
) {
    loop {
        if let Err(e) = subscribe_once(&client, channel, &local).await {
            tracing::warn!("[event_bus] {} の購読が切れました: {}", channel, e);
        }
        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
    }
}

async fn subscribe_once<T: DeserializeOwned>(
    client: &redis::Client,
    channel: &'static str,
    local: &broadcast::Sender<T>,
) -> Result<(), String> {
    let mut pubsub = client
        .get_async_pubsub()
        .await
        .map_err(|e| format!("接続に失敗: {}", e))?;
    pubsub
        .subscribe(channel)
        .await
        .map_err(|e| format!("購読に失敗: {}", e))?;
    let mut messages = pubsub.on_message();
    while let Some(msg) = messages.next().await {
        let payload: String = match msg.get_payload() {
            Ok(payload) => payload,
            Err(e) => {
                tracing::error!("[event_bus] {} のメッセージ取得に失敗: {}", channel, e);
                continue;
            }
        };
        match serde_json::from_str::<T>(&payload) {
            Ok(event) => {
                let _ = local.send(event);
            }
            Err(e) => tracing::error!("[event_bus] {} のデシリアライズに失敗: {}", channel, e),
        }
    }
    Err("購読ストリームが終了しました".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_bus_delivers_to_subscribers() {
        let bus = EventBus::<String>::local(4);
        let mut rx = bus.subscribe();
        bus.publish("hello".to_string());
        assert_eq!(rx.recv().await.unwrap(), "hello");
    }
}
