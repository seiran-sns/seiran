//! リアルタイム更新のためのストリーミングハブ（#37）。
//!
//! ローカルで発生したイベント（新規ポスト・リアクション・フォロー等）を、
//! 受け取るべきローカルアクターの WebSocket 接続へブロードキャストする。
//! フィルタは各接続側で `recipients` を見て行う。
//!
//! mono バイナリでは api ロールと federation ロールが同一プロセスで動くため、
//! この共有ハブ 1 つを両者の状態に注入して跨いで配信する。

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

    /// 任意種別のイベントを送出する。`{"type":<kind>,"body":<body>}` として配信する。
    pub fn publish_event(&self, recipients: HashSet<i64>, kind: &str, body: serde_json::Value) {
        if recipients.is_empty() {
            return;
        }
        let payload = serde_json::json!({ "type": kind, "body": body }).to_string();
        self.publish(StreamEvent {
            recipients: Arc::new(recipients),
            payload: Arc::new(payload),
        });
    }

    /// 新規ポストイベント（`type: "note"`）を送出する。
    pub fn publish_note(&self, recipients: HashSet<i64>, note_json: &serde_json::Value) {
        self.publish_event(recipients, "note", note_json.clone());
    }
}
