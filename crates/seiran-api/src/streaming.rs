//! リアルタイム更新のためのストリーミングハブ（#37）。
//!
//! ローカルで発生したイベント（新規ポスト等）を、受け取るべきローカルアクターの
//! WebSocket 接続へブロードキャストする。フィルタは各接続側で `recipients` を見て行う。

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::broadcast;

/// ストリーミングイベント。`recipients` に含まれるローカルアクターのみが受信する。
#[derive(Clone)]
pub struct StreamEvent {
    pub recipients: Arc<HashSet<i64>>,
    /// クライアントへ送る JSON テキスト（例: `{"type":"note","body":{...}}`）。
    pub payload: Arc<String>,
}

/// プロセス内共有のブロードキャストハブ。
pub struct StreamHub {
    tx: broadcast::Sender<StreamEvent>,
}

impl Default for StreamHub {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(512);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StreamEvent> {
        self.tx.subscribe()
    }

    /// イベントを送出する（購読者がいなくてもエラーにしない）。
    pub fn publish(&self, ev: StreamEvent) {
        let _ = self.tx.send(ev);
    }

    /// 新規ポストイベントを送出する。`recipients` は受信すべきローカルアクター ID 集合。
    pub fn publish_note(&self, recipients: HashSet<i64>, note_json: &serde_json::Value) {
        if recipients.is_empty() {
            return;
        }
        let payload = serde_json::json!({ "type": "note", "body": note_json }).to_string();
        self.publish(StreamEvent {
            recipients: Arc::new(recipients),
            payload: Arc::new(payload),
        });
    }
}
